use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use rmcp::transport::stdio;
use rmcp::ServiceExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use search_gateway::app::App;
use search_gateway::config::{self, Config, LogConfig};
use search_gateway::mcp::GatewayServer;
use search_gateway::model::DoctorOverallStatus;
use search_gateway::selftest;

#[derive(Debug, Default)]
struct Args {
    config: Option<PathBuf>,
    doctor: bool,
    selftest: bool,
    live: bool,
    live_providers: bool,
    help: bool,
    version: bool,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut args = Self::default();
        let mut iter = std::env::args().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--config" => {
                    let value = iter.next().context("--config requires a path")?;
                    args.config = Some(PathBuf::from(value));
                }
                "--doctor" => args.doctor = true,
                "--selftest" => args.selftest = true,
                "--live" => args.live = true,
                "--live-providers" => args.live_providers = true,
                "--help" | "-h" => args.help = true,
                "--version" | "-V" => args.version = true,
                other => bail!("unknown argument: {other}"),
            }
        }
        if args.doctor && args.selftest {
            bail!("--doctor and --selftest are mutually exclusive");
        }
        Ok(args)
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse()?;
    if args.help {
        print_help();
        return Ok(());
    }
    if args.version {
        println!("search-gateway {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async_main(args))
}

async fn async_main(args: Args) -> anyhow::Result<()> {
    let loaded = if args.doctor || args.selftest {
        Config::load_lenient(args.config.as_deref())?
    } else {
        Config::load(args.config.as_deref())?
    };
    let _guard = init_logging(&loaded.config.log);
    for warning in &loaded.warnings {
        tracing::warn!("{warning}");
    }

    if args.selftest {
        let mut loaded = loaded;
        if !args.live_providers && !args.live {
            loaded.config.tavily.enabled = false;
            loaded.config.firecrawl.enabled = false;
        }
        if !loaded.config.searxng.enabled {
            bail!("selftest requires searxng.enabled = true when --live-providers is not set");
        }
        let app = App::new_lenient(loaded)?;
        let report = selftest::run(&app, args.live_providers || args.live).await;
        println!("{}", serde_json::to_string_pretty(&report)?);
        if !report.passed {
            std::process::exit(1);
        }
        return Ok(());
    }

    if args.doctor {
        let app = App::new_lenient(loaded)?;
        let report = app.doctor.run(args.live).await;
        let failed = matches!(
            report.status,
            DoctorOverallStatus::Unreachable | DoctorOverallStatus::Misconfigured
        );
        println!("{}", serde_json::to_string_pretty(&report)?);
        if failed {
            std::process::exit(1);
        }
        return Ok(());
    }

    let app = App::new(loaded)?;
    let server = GatewayServer::new(app);
    let service = server
        .serve(stdio())
        .await
        .context("failed to start MCP stdio server")?;
    service.waiting().await.context("MCP server stopped")?;
    Ok(())
}

fn init_logging(cfg: &LogConfig) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&cfg.level))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);

    if cfg.file {
        let directory = config::default_config_path()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("logs");
        if let Err(error) = std::fs::create_dir_all(&directory) {
            eprintln!("cannot create log directory {}: {error}", directory.display());
            tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .with_env_filter(filter)
                .init();
            return None;
        }
        let file = tracing_appender::rolling::never(&directory, "gateway.log");
        let (writer, guard) = tracing_appender::non_blocking(file);
        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_ansi(false);
        tracing_subscriber::registry()
            .with(filter)
            .with(stderr_layer)
            .with(file_layer)
            .init();
        return Some(guard);
    }

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();
    None
}

fn print_help() {
    println!(
        "search-gateway {}\n\
         \n\
         Retrieval-only MCP server for Tavily, Firecrawl and SearXNG.\n\
         \n\
         Usage:\n\
           search-gateway [--config PATH]            run the MCP stdio server\n\
           search-gateway --doctor [--live]          run diagnostics\n\
           search-gateway --selftest [--live-providers]\n\
         \n\
         Options:\n\
           --config PATH         configuration file (default: %APPDATA%\\search-gateway\\config.toml)\n\
           --doctor              run diagnostics and exit\n\
           --selftest            run the 10-query golden set and exit\n\
           --live                doctor: probe Tavily, Firecrawl and a free fetch\n\
           --live-providers      selftest: enable Tavily and Firecrawl\n\
           -h, --help            print this help\n\
           -V, --version         print the version",
        env!("CARGO_PKG_VERSION")
    );
}
