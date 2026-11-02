// GUI-subsystem exe so double-clicking opens no console window. The CLI modes attach to the
// parent console explicitly (see `cli::attach_console`).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod canvas;
mod cli;
mod core_api;
mod recent;

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match cli::parse(&args) {
        Ok(c) => c,
        Err(e) => {
            cli::attach_console();
            eprintln!("error: {e}\n\n{}", cli::HELP);
            std::process::exit(2);
        }
    };
    match cmd {
        cli::Command::Gui(path) => {
            if let Err(e) = run_gui(path) {
                cli::attach_console();
                eprintln!("error: cannot start the window: {e}");
                std::process::exit(1);
            }
        }
        other => {
            cli::attach_console();
            let code = cli::run(other);
            std::process::exit(code);
        }
    }
}

fn run_gui(initial: Option<PathBuf>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("short-circ")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([480.0, 320.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("short-circ", options, Box::new(move |cc| Ok(Box::new(app::ShortCircApp::new(cc, initial)))))
}
