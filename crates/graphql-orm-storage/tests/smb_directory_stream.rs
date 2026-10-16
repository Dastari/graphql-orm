//! Actual SDK query regressions for the test-owned Samba fixture.
//! Never use against a live or application-owned share.
#![cfg(feature = "smb")]
use futures_util::StreamExt;
use smb::{Client, ClientConfig, ConnectionConfig, Directory, FileCreateArgs, UncPath};
use smb_fscc::{
    DirAccessMask, FileAccessMask, FileAttributes, FileDispositionInformation,
    FileFullDirectoryInformation,
};
use smb_msg::CreateOptions;
use std::{collections::BTreeSet, env, error::Error, io, sync::Arc, time::Duration};
use tokio::time::timeout;
use uuid::Uuid;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const BUFFER: u32 = 0x300;
const COUNT: usize = 32;
fn failure(message: &str) -> Box<dyn Error> {
    io::Error::other(message).into()
}
fn required(name: &str) -> Result<String> {
    Ok(env::var(name)?)
}

struct Fixture {
    client: Client,
    root: UncPath,
    root_name: String,
    files: Vec<String>,
    directory_created: bool,
}
impl Fixture {
    async fn connect() -> Result<Self> {
        // The wrapper must separately bind this run ID to the owned container.
        let run = required("SMB_TEST_FIXTURE_RUN")?;
        if run.len() != 32 || !run.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(failure("invalid owned fixture identity"));
        }
        if required("SMB_TEST_SERVER")? != "127.0.0.1" {
            return Err(failure("only wrapper-owned loopback Samba is permitted"));
        }
        let port: u16 = required("SMB_TEST_PORT")?.parse()?;
        if port == 0 || port == 445 || port == 139 {
            return Err(failure("expected mapped fixture port"));
        }
        let mut connection = ConnectionConfig {
            port: Some(port),
            timeout: Some(Duration::from_secs(5)),
            allow_unsigned_guest_access: false,
            smb2_only_negotiate: true,
            ..Default::default()
        };
        connection.auth_methods.kerberos = false;
        connection.auth_methods.ntlm = true;
        let client = Client::new(ClientConfig {
            connection,
            ..Default::default()
        });
        let share = UncPath::new("127.0.0.1")?.with_share(&required("SMB_TEST_SHARE")?)?;
        let user = required("SMB_TEST_USERNAME")?;
        let user = match env::var("SMB_TEST_DOMAIN") {
            Ok(domain) if !domain.is_empty() => format!("{domain}\\{user}"),
            _ => user,
        };
        timeout(
            Duration::from_secs(10),
            client.share_connect(&share, &user, required("SMB_TEST_PASSWORD")?),
        )
        .await??;
        let root_name = format!("sdk-stream-{}", Uuid::new_v4());
        Ok(Self {
            client,
            root: share.with_path(&root_name),
            root_name,
            files: vec![],
            directory_created: false,
        })
    }
    async fn populate(&mut self) -> Result<()> {
        let directory = self
            .client
            .create_file(
                &self.root,
                &FileCreateArgs::make_create_new(
                    FileAttributes::new().with_directory(true),
                    CreateOptions::new().with_directory_file(true),
                ),
            )
            .await?
            .unwrap_dir();
        self.directory_created = true;
        directory.close().await?;
        for index in 0..COUNT {
            let name = format!("entry-{index:03}-{}", "x".repeat(96));
            let path = self
                .root
                .clone()
                .with_path(&format!("{}\\{name}", self.root_name));
            let file = self
                .client
                .create_file(
                    &path,
                    &FileCreateArgs::make_create_new(Default::default(), Default::default()),
                )
                .await?
                .unwrap_file();
            self.files.push(name);
            file.close().await?;
        }
        Ok(())
    }
    async fn open_directory(&self) -> Result<Arc<Directory>> {
        Ok(Arc::new(
            self.client
                .create_file(
                    &self.root,
                    &FileCreateArgs::make_open_existing(
                        DirAccessMask::new()
                            .with_list_directory(true)
                            .with_synchronize(true)
                            .into(),
                    ),
                )
                .await?
                .unwrap_dir(),
        ))
    }
    async fn cleanup(&self) -> Result<()> {
        // Try every task-owned path even after an earlier cleanup error.
        let mut first = None;
        for name in &self.files {
            let path = self
                .root
                .clone()
                .with_path(&format!("{}\\{name}", self.root_name));
            let outcome: Result<()> = async {
                let file = self
                    .client
                    .create_file(
                        &path,
                        &FileCreateArgs::make_open_existing(
                            FileAccessMask::new().with_delete(true),
                        ),
                    )
                    .await?
                    .unwrap_file();
                let disposition = file
                    .set_info(FileDispositionInformation {
                        delete_pending: true.into(),
                    })
                    .await;
                let close = file.close().await;
                disposition?;
                close?;
                Ok(())
            }
            .await;
            if let Err(error) = outcome
                && first.is_none()
            {
                first = Some(error);
            }
        }
        if self.directory_created {
            let outcome: Result<()> = async {
                let dir = self
                    .client
                    .create_file(
                        &self.root,
                        &FileCreateArgs::make_open_existing(
                            FileAccessMask::new().with_delete(true),
                        ),
                    )
                    .await?
                    .unwrap_dir();
                let disposition = dir
                    .set_info(FileDispositionInformation {
                        delete_pending: true.into(),
                    })
                    .await;
                let close = dir.close().await;
                disposition?;
                close?;
                Ok(())
            }
            .await;
            if let Err(error) = outcome
                && first.is_none()
            {
                first = Some(error);
            }
        }
        if let Err(error) = self.client.close().await
            && first.is_none()
        {
            first = Some(error.into());
        }
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

async fn exercise(drop_early: bool) -> Result<()> {
    let mut fixture = Fixture::connect().await?;
    let result: Result<()> = async {
        fixture.populate().await?;
        let directory = fixture.open_directory().await?;
        let mut stream = Directory::query_with_options::<FileFullDirectoryInformation>(&directory, "entry-*", BUFFER).await?;
        if drop_early {
            let first = timeout(Duration::from_secs(20), stream.next()).await?
                .ok_or_else(|| failure("expected first directory entry"))??;
            if !fixture.files.contains(&first.file_name.to_string()) { return Err(failure("unexpected first filename")); }
            // Observed: an actual server entry arrived while query stream is live.
            // Public API exposes neither batch depth nor producer idle state.
            // This is bounded drop-liveness, NOT deterministic idle/race proof.
            let weak = Arc::downgrade(&directory);
            drop(stream);
            drop(directory);
            timeout(Duration::from_secs(5), async {
                let mut poll = tokio::time::interval(Duration::from_millis(10));
                while weak.strong_count() != 0 { poll.tick().await; }
            }).await.map_err(|_| failure("dropped query retained Directory Arc"))?;
            println!("SDK_DROP_LIVENESS_PASS observed_first_entry=true idle_observed=false");
        } else {
            let names: Vec<String> = timeout(Duration::from_secs(20), async {
                let mut names = vec![];
                while let Some(item) = stream.next().await {
                    names.push(item?.file_name.to_string());
                    if names.len() > COUNT { return Err(failure("duplicate or unexpected extra entry")); }
                }
                Ok::<_, Box<dyn Error>>(names)
            }).await??;
            drop(stream);
            let close = directory.close().await;
            drop(directory);
            close?;
            let actual: BTreeSet<_> = names.iter().cloned().collect();
            let expected: BTreeSet<_> = fixture.files.iter().cloned().collect();
            if names.len() != COUNT || actual.len() != COUNT || actual != expected {
                return Err(failure("directory EOF filenames differ or contain duplicates"));
            }
            // UTF-16 filename bytes alone exceed one requested response buffer.
            // Thus exact completion must traverse multiple real server batches.
            let minimum_name_bytes: usize = names.iter().map(|name| name.encode_utf16().count() * 2).sum();
            if minimum_name_bytes <= BUFFER as usize { return Err(failure("multibatch precondition not established")); }
            println!("SDK_MULTIBATCH_EOF_PASS count={COUNT} buffer={BUFFER} minimum_name_bytes={minimum_name_bytes}");
        }
        Ok(())
    }.await;
    let cleanup = timeout(Duration::from_secs(30), fixture.cleanup()).await;
    // No panic/assertion before explicit cleanup. Wrapper-owned service/volume
    // teardown remains the fallback on unexpected panic/process timeout.
    match cleanup {
        Ok(Ok(())) => println!("SDK_OWNED_PATH_CLEANUP_PASS"),
        Ok(Err(error)) => return Err(error),
        Err(error) => return Err(error.into()),
    }
    result
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires wrapper-owned Samba and explicit SMB_TEST_* fixture identity"]
async fn sdk_directory_multibatch_eof_exact_names() -> Result<()> {
    exercise(false).await
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires wrapper-owned Samba and explicit SMB_TEST_* fixture identity"]
async fn sdk_directory_drop_releases_directory_arc() -> Result<()> {
    exercise(true).await
}
