use std::path::PathBuf;

use aitools_cli::{
    analyze_logs_dir, audit_plugin, export_logs, import_diag_to_ztrace, init_plugin,
    migrate_legacy_plugin, pack_plugin, replay_ztrace, serve_mcp_stdio, PluginTemplate,
};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "aitools", version, about = "aitools developer CLI & harness")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Plugin {
        #[command(subcommand)]
        action: PluginCommands,
    },
    Logs {
        #[command(subcommand)]
        action: LogsCommands,
    },
    Mcp {
        #[command(subcommand)]
        action: McpCommands,
    },
    Harness {
        #[command(subcommand)]
        action: HarnessCommands,
    },
    Doctor {
        #[arg(long)]
        log_dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum PluginCommands {
    Init {
        name: String,
        #[arg(long, value_enum, default_value_t = PluginTemplate::NativeUi)]
        template: PluginTemplate,
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    Audit {
        dir: PathBuf,
    },
    Pack {
        dir: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        sign: Option<PathBuf>,
    },
    Migrate {
        dir: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum LogsCommands {
    Export {
        #[arg(long)]
        log_dir: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        zip: bool,
        #[arg(long)]
        trace_id: Option<String>,
    },
    Analyze {
        #[arg(long)]
        log_dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum McpCommands {
    Serve {
        #[arg(long, default_value = "stdio")]
        transport: String,
    },
}

#[derive(Subcommand)]
enum HarnessCommands {
    Replay {
        file: PathBuf,
        #[arg(long)]
        assert_no_anomalies: bool,
    },
    ImportDiag {
        zip: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        trace_id: Option<String>,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Plugin { action } => match action {
            PluginCommands::Init {
                name,
                template,
                dir,
            } => {
                let target = dir.unwrap_or_else(|| PathBuf::from(&name));
                init_plugin(&target, &name, template)?;
                println!("Initialized plugin at {}", target.display());
            }
            PluginCommands::Audit { dir } => {
                let report = audit_plugin(&dir)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            PluginCommands::Pack { dir, out, sign } => {
                let id = pack_plugin(&dir, &out, sign.as_deref())?;
                println!("Packed {id} -> {}", out.display());
            }
            PluginCommands::Migrate { dir, out } => {
                let report = migrate_legacy_plugin(&dir, out.as_deref())?;
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
        },
        Commands::Logs { action } => match action {
            LogsCommands::Export {
                log_dir,
                out,
                zip,
                trace_id,
            } => {
                let summary = export_logs(log_dir, &out, zip, trace_id)?;
                println!("{}", serde_json::to_string_pretty(&summary)?);
            }
            LogsCommands::Analyze { log_dir } => {
                let report = analyze_logs_dir(log_dir)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
        },
        Commands::Mcp { action } => match action {
            McpCommands::Serve { transport } => {
                if transport != "stdio" {
                    return Err(format!("unsupported transport '{transport}'; use stdio").into());
                }
                serve_mcp_stdio()?;
            }
        },
        Commands::Harness { action } => match action {
            HarnessCommands::Replay {
                file,
                assert_no_anomalies,
            } => {
                let report = replay_ztrace(&file, assert_no_anomalies)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            HarnessCommands::ImportDiag { zip, out, trace_id } => {
                let count = import_diag_to_ztrace(&zip, &out, trace_id.as_deref())?;
                println!("Imported {count} trace records -> {}", out.display());
            }
        },
        Commands::Doctor { log_dir } => {
            let report = analyze_logs_dir(log_dir)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}
