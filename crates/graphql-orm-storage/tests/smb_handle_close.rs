//! Vendored SMB client handle and client close regressions for the
//! test-owned Samba fixture. Never use against a live or application-owned
//! share.
#![cfg(feature = "smb")]
use smb::{
    Client, ClientConfig, ConnectionConfig, Error as SmbError, File, FileCreateArgs, UncPath,
};
use smb_fscc::{FileAccessMask, FileAttributes, FileDispositionInformation};
use smb_msg::{ShareAccessFlags, Status};
use std::{env, error::Error, io, sync::Arc, time::Duration};
use tokio::time::{Instant, sleep, timeout};
use uuid::Uuid;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Credits the connection asks the server to keep granted. A read whose
/// credit charge is larger can never acquire its credits, so it stays queued
/// at the head of the connection's fair credit semaphore and every later
/// request waits behind it.
const CREDIT_BACKLOG: u16 = 8;
/// 4 MiB costs 64 credits (one per 64 KiB), well above the backlog.
const STALLED_READ_BYTES: usize = 4 * 1024 * 1024;

fn failure(message: &str) -> Box<dyn Error> {
    io::Error::other(message).into()
}

fn required(name: &str) -> Result<String> {
    Ok(env::var(name)?)
}

async fn connect(credits_backlog: Option<u16>) -> Result<(Client, UncPath)> {
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
        credits_backlog,
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
    Ok((client, share))
}

async fn create_file(client: &Client, path: &UncPath, contents: &[u8]) -> Result<()> {
    let file = client
        .create_file(
            path,
            &FileCreateArgs::make_create_new(FileAttributes::new(), Default::default()),
        )
        .await?
        .unwrap_file();
    let written = if contents.is_empty() {
        Ok(0)
    } else {
        file.write_block(contents, 0, None).await
    };
    let close = file.close().await;
    written?;
    close?;
    Ok(())
}

async fn open_file(client: &Client, path: &UncPath, exclusive: bool) -> smb::Result<File> {
    let mut args = FileCreateArgs::make_open_existing(
        FileAccessMask::new()
            .with_file_read_data(true)
            .with_file_read_attributes(true)
            .with_synchronize(true),
    );
    if exclusive {
        args.share_access = Some(ShareAccessFlags::new());
    }
    Ok(client.create_file(path, &args).await?.unwrap_file())
}

fn is_sharing_violation(error: &SmbError) -> bool {
    matches!(
        error,
        SmbError::ReceivedErrorMessage(status, _) | SmbError::UnexpectedMessageStatus(status)
            if *status == Status::SharingViolation as u32
    )
}

async fn delete_file(client: &Client, path: &UncPath) -> Result<()> {
    let file = client
        .create_file(
            path,
            &FileCreateArgs::make_open_existing(FileAccessMask::new().with_delete(true)),
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

async fn cancelled_close_scenario(
    client: &Client,
    held: &UncPath,
    staller: &UncPath,
) -> Result<()> {
    create_file(client, held, b"").await?;
    create_file(client, staller, b"x").await?;
    // Shares read, write and delete, so an exclusive open fails while it is open.
    let handle = open_file(client, held, false).await?;

    // A read whose credit charge the connection can never grant waits at the
    // head of the fair credit semaphore; every later request queues behind it.
    let stall = Arc::new(open_file(client, staller, false).await?);
    let stalled_read = tokio::spawn({
        let stall = stall.clone();
        async move {
            let mut buffer = vec![0u8; STALLED_READ_BYTES];
            stall.read_block(&mut buffer, 0, None, false).await
        }
    });
    sleep(Duration::from_millis(500)).await;
    if stalled_read.is_finished() {
        return Err(failure(
            "precondition: the oversized read was expected to wait for credits",
        ));
    }

    // The close waits for a credit and is cancelled before CLOSE is sent.
    if timeout(Duration::from_secs(1), handle.close())
        .await
        .is_ok()
    {
        return Err(failure(
            "precondition: close was expected to wait for a credit behind the stalled read",
        ));
    }

    stalled_read.abort();
    if !stalled_read.await.is_err_and(|error| error.is_cancelled()) {
        return Err(failure("the stalled read was expected to be cancelled"));
    }

    // The cancelled close never reached the server: the handle is still open.
    match open_file(client, held, true).await {
        Err(error) if is_sharing_violation(&error) => {}
        Err(error) => return Err(error.into()),
        Ok(_) => {
            return Err(failure(
                "the server handle was released although CLOSE was never sent",
            ));
        }
    }

    // Dropping the handle after the cancelled close must release it.
    drop(handle);
    let deadline = Instant::now() + Duration::from_secs(5);
    let exclusive = loop {
        match open_file(client, held, true).await {
            Ok(file) => break file,
            Err(error) if is_sharing_violation(&error) && Instant::now() < deadline => {
                sleep(Duration::from_millis(100)).await;
            }
            Err(error) if is_sharing_violation(&error) => {
                return Err(failure(
                    "a close cancelled while waiting for a credit leaked the server handle",
                ));
            }
            Err(error) => return Err(error.into()),
        }
    };
    exclusive.close().await?;
    Arc::try_unwrap(stall)
        .map_err(|_| failure("the stalled read still holds its handle"))?
        .close()
        .await?;
    println!("SMB_CANCELLED_CLOSE_RELEASED_BY_DESTRUCTOR");
    Ok(())
}

/// #710: a close cancelled while it waits for a credit stays the handle's
/// destructor's responsibility, so the server handle is released and the
/// next exclusive open of the path succeeds on the same session.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires wrapper-owned Samba and explicit SMB_TEST_* fixture identity"]
async fn samba_a_close_cancelled_while_waiting_for_a_credit_releases_the_handle() -> Result<()> {
    let (client, share) = connect(Some(CREDIT_BACKLOG)).await?;
    let run = Uuid::new_v4();
    let held = share.clone().with_path(&format!("close-cancel-{run}.bin"));
    let staller = share
        .clone()
        .with_path(&format!("close-cancel-{run}-stall.bin"));
    let result = timeout(
        Duration::from_secs(60),
        cancelled_close_scenario(&client, &held, &staller),
    )
    .await
    .unwrap_or_else(|_| Err(failure("the scenario did not finish")));
    // Try every task-owned path even after an earlier failure.
    let mut cleanup = Ok(());
    for path in [&held, &staller] {
        if let Err(error) = timeout(Duration::from_secs(10), delete_file(&client, path))
            .await
            .unwrap_or_else(|_| Err(failure("cleanup timed out")))
            && cleanup.is_ok()
        {
            cleanup = Err(error);
        }
    }
    let close = client.close().await;
    result?;
    cleanup?;
    close?;
    Ok(())
}

/// #710: `Client::close` reports a failed logoff or connection close instead
/// of returning success.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires wrapper-owned Samba and explicit SMB_TEST_* fixture identity"]
async fn samba_client_close_reports_a_failed_session_and_connection_close() -> Result<()> {
    let (client, share) = connect(None).await?;
    // Disconnect the tree cleanly, then stop the connection under the
    // session, so only the logoff and the connection close can fail.
    client.get_tree(&share).await?.disconnect().await?;
    client.get_connection("127.0.0.1").await?.close().await?;
    match client.close().await {
        Err(error) => println!("SMB_CLIENT_CLOSE_REPORTED {error}"),
        Ok(()) => {
            return Err(failure(
                "Client::close returned success although the logoff and connection close failed",
            ));
        }
    }
    // Every step was attempted and the client holds nothing afterwards.
    client.close().await?;
    Ok(())
}
