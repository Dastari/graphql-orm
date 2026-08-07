use std::{path::PathBuf, process::ExitCode};

use graphql_orm_router::RouterFileConfig;

const CONFIG_ENV: &str = "GRAPHQL_ORM_ROUTER_CONFIG";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("graphql-orm-router: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let arguments = parse_arguments()?;
    if arguments.help {
        println!(
            "Usage: graphql-orm-router --config <router.json> [--check]\n\
             \n\
             GRAPHQL_ORM_ROUTER_CONFIG may supply the path. Listener overrides use\n\
             GRAPHQL_ORM_ROUTER_LISTENER and GRAPHQL_ORM_ROUTER_ADMIN_LISTENER."
        );
        return Ok(());
    }
    let path = arguments
        .config
        .or_else(|| std::env::var_os(CONFIG_ENV).map(PathBuf::from))
        .ok_or_else(|| format!("a --config path or {CONFIG_ENV} is required"))?;
    let config = RouterFileConfig::load_json(path)
        .and_then(RouterFileConfig::into_router_config)
        .map_err(|error| error.to_string())?;

    if arguments.check {
        hive_router::ntex::rt::System::build()
            .name("graphql-orm-router-check")
            .build(hive_router::ntex::rt::DefaultRuntime)
            .block_on(async move {
                let prepared = config.prepare().await.map_err(|error| error.to_string())?;
                println!(
                    "configuration ready: graph_version={} graph_fingerprint={}",
                    prepared.active_graph().version(),
                    prepared.active_graph().fingerprint()
                );
                Ok(())
            })
    } else {
        graphql_orm_router::run(config).map_err(|error| error.to_string())
    }
}

#[derive(Default)]
struct Arguments {
    config: Option<PathBuf>,
    check: bool,
    help: bool,
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut result = Arguments::default();
    let mut values = std::env::args_os().skip(1);
    while let Some(value) = values.next() {
        match value.to_str() {
            Some("--config") => {
                if result.config.is_some() {
                    return Err("--config may be supplied only once".to_owned());
                }
                result.config = Some(
                    values
                        .next()
                        .map(PathBuf::from)
                        .ok_or_else(|| "--config requires a path".to_owned())?,
                );
            }
            Some("--check") => result.check = true,
            Some("--help" | "-h") => result.help = true,
            _ => return Err("unknown or non-UTF-8 command-line argument".to_owned()),
        }
    }
    Ok(result)
}
