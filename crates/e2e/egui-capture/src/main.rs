//! Render, don't approximate: capture the *actual* `egui_wgpu` client UI.
//!
//! ```sh
//! cargo run -p openshard-egui-capture -- \
//!   --client "/path/to/Ultima Online Classic" --out /tmp/craft-catalogue.png
//! ```
//!
//! The tool builds the playground, starts it as a child of a private headless
//! Sway compositor, and has that compositor's `grim` capture the output.  The
//! catalogue comes from the live in-process shard; neither the canvas nor its
//! items are reimplemented for the screenshot.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::Parser;

/// Render a real Craft Catalogue egui frame to PNG without touching the active
/// desktop session.
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Client install that supplies the map, art, and localization tables.
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,

    /// Destination PNG. Parent directories are made when needed.
    #[arg(short, long, value_name = "PNG")]
    out: PathBuf,

    /// Logical output width presented to the real client.
    #[arg(long, default_value_t = 1440)]
    width: u32,

    /// Logical output height presented to the real client.
    #[arg(long, default_value_t = 900)]
    height: u32,

    /// Time for startup, login, live catalogue reply, and egui frames before capture.
    #[arg(long, default_value_t = 8)]
    wait_seconds: u64,

    /// Query prefilled only in the isolated capture process. This makes the
    /// screenshot demonstrate a focused class of recipes without changing the
    /// normal catalogue opening state.
    #[arg(long, default_value = "sword")]
    showcase_query: String,

    /// Move the private Sway cursor over the first result before capture, so
    /// the PNG also proves the real egui tooltip path.
    #[arg(long)]
    hover_first_result: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.width == 0 || cli.height == 0 {
        eprintln!("--width and --height must be greater than zero");
        return ExitCode::FAILURE;
    }
    let out = absolute_path(&cli.out);
    if let Some(parent) = out.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!("creating {}: {error}", parent.display());
            return ExitCode::FAILURE;
        }
    }

    let workspace = match workspace_root() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    if !build_playground(&workspace) {
        return ExitCode::FAILURE;
    }
    let playground = workspace
        .join("target")
        .join("debug")
        .join("openshard-playground");
    if !playground.is_file() {
        eprintln!("{} was not built", playground.display());
        return ExitCode::FAILURE;
    }

    let scratch = std::env::temp_dir().join(format!("openshard-egui-capture-{}", std::process::id()));
    if let Err(error) = std::fs::create_dir_all(&scratch) {
        eprintln!("creating {}: {error}", scratch.display());
        return ExitCode::FAILURE;
    }
    let config = scratch.join("sway.conf");
    let capture = scratch.join("capture.sh");
    if let Err(error) = std::fs::write(&capture, capture_script(&out, &cli)) {
        eprintln!("writing {}: {error}", capture.display());
        let _ = std::fs::remove_dir_all(&scratch);
        return ExitCode::FAILURE;
    }
    let contents = sway_config(&playground, &cli.client, &capture, &cli);
    if let Err(error) = std::fs::write(&config, contents) {
        eprintln!("writing {}: {error}", config.display());
        let _ = std::fs::remove_dir_all(&scratch);
        return ExitCode::FAILURE;
    }

    let status = Command::new("sway")
        .arg("-c")
        .arg(&config)
        // Sway creates a distinct Wayland socket for its own children.  Do not
        // pass SWAYSOCK/WAYLAND_DISPLAY from a user's active desktop.
        .env("WLR_BACKENDS", "headless")
        .env("WLR_RENDERER_ALLOW_SOFTWARE", "1")
        .status();
    let _ = std::fs::remove_dir_all(&scratch);
    match status {
        Ok(status) if status.success() && out.is_file() => {
            println!("wrote {}", out.display());
            ExitCode::SUCCESS
        }
        Ok(status) => {
            eprintln!("headless Sway exited with {status}");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("starting sway (requires sway and grim): {error}");
            ExitCode::FAILURE
        }
    }
}

fn workspace_root() -> Result<PathBuf, String> {
    let output = Command::new("cargo")
        .args(["locate-project", "--workspace", "--message-format", "plain"])
        .output()
        .map_err(|error| format!("locating workspace: {error}"))?;
    if !output.status.success() {
        return Err("cargo locate-project could not find the workspace root".to_owned());
    }
    let manifest = String::from_utf8(output.stdout)
        .map_err(|error| format!("cargo locate-project returned invalid UTF-8: {error}"))?;
    PathBuf::from(manifest.trim())
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "workspace manifest has no parent directory".to_owned())
}

fn build_playground(workspace: &Path) -> bool {
    matches!(
        Command::new("cargo")
            .args(["build", "-p", "openshard-playground"])
            .current_dir(workspace)
            .status(),
        Ok(status) if status.success()
    )
}

fn sway_config(playground: &Path, client: &Path, capture: &Path, cli: &Cli) -> String {
    let app = format!(
        "env OPENSHARD_CRAFT_CATALOGUE_SHOWCASE_QUERY={} {} --client {} --scenario craft-catalogue",
        shell_quote_text(&cli.showcase_query),
        shell_quote(playground.as_os_str()),
        shell_quote(client.as_os_str()),
    );
    format!(
        "output HEADLESS-1 resolution {}x{}\nexec sh {}\nexec {}\n",
        cli.width,
        cli.height,
        shell_quote(capture.as_os_str()),
        app
    )
}

fn capture_script(out: &Path, cli: &Cli) -> String {
    let hover = match cli.hover_first_result {
        true => {
            "swaymsg seat seat0 cursor set 88 358\nsleep 1\nswaymsg seat seat0 cursor set 95 365\nsleep 2\n"
        }
        false => "",
    };
    format!(
        "#!/bin/sh\nsleep {}\n{hover}grim {}\nswaymsg exit\n",
        cli.wait_seconds,
        shell_quote(out.as_os_str()),
    )
}

fn shell_quote(value: &std::ffi::OsStr) -> String {
    shell_quote_text(&value.to_string_lossy())
}

fn shell_quote_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\\"'\\\"'"))
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}
