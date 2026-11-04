//! `bridle`: the Bridle program. Run it in a folder and a window opens on that folder; give it a command (`bridle doctor`,
//! `bridle models serve main`) and it runs that command in the terminal, as the headless `bridle-cli` does.
//!
//! It is a console program so that commands print where they are run. The window runs in a second, detached copy of the
//! same program (`--gui`), which has no console, so the terminal is free again at once and no console window is left
//! behind. The detached copy is what `bridle.exe` shows in the task manager while the window is open.
//!
//! The window is another front end to the same agent loop, policy, tools and event log as the `bridle`
//! command and its terminal UI. It has no privileges of its own: it can call the commands in
//! `bridge.rs` and listen to the `bridle` event, and nothing else (see `capabilities/default.json`).

mod bridge;
mod crew_cmd;
mod ideas_cmd;
mod view;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;

use harness_cli::tui::data::LoadArgs;
use tauri::{Manager, RunEvent};

const USAGE: &str = "bridle [FOLDER] [--config FILE]\n\
Opens the Bridle window on FOLDER (default: the current folder).\n\
bridle COMMAND ...   runs a command in the terminal; `bridle --help` lists them.";
/// The flag the detached copy that shows the window is started with. It is not for people to type.
const GUI_FLAG: &str = "--gui";

/// The command line: an optional folder and an optional explicit config file.
fn parse_args(mut args: impl Iterator<Item = OsString>) -> Result<LoadArgs, String> {
    let mut loaded = LoadArgs::default();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--config") => {
                let file = args.next().ok_or("--config needs a file")?;
                loaded.config = Some(PathBuf::from(file));
            }
            Some("--help" | "-h") => return Err(USAGE.to_owned()),
            Some(flag) if flag.starts_with("--") => {
                return Err(format!("unknown option `{flag}`\n{USAGE}"));
            }
            _ => {
                if loaded.workspace.is_some() {
                    return Err(format!("only one folder can be given\n{USAGE}"));
                }
                loaded.workspace = Some(PathBuf::from(arg));
            }
        }
    }
    Ok(loaded)
}

/// Whether the command line is a command for the terminal (`doctor`, `models ...`, `--help`) and not a folder to open.
/// The options that come before a command (`--config FILE`, `--workspace FOLDER`) are skipped.
fn wants_command_line(args: &[OsString]) -> bool {
    let mut words = args.iter().filter_map(|a| a.to_str());
    while let Some(word) = words.next() {
        match word {
            "--config" | "--workspace" => {
                words.next();
            }
            "--help" | "-h" | "--version" | "-V" => return true,
            GUI_FLAG => {}
            other if other.starts_with('-') => {}
            other => return harness_cli::cli::is_subcommand(other),
        }
    }
    false
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if wants_command_line(&args) {
        return harness_cli::cli::run();
    }
    let is_window_copy = args.first().is_some_and(|a| a == GUI_FLAG);
    let window_args: Vec<OsString> = if is_window_copy {
        args[1..].to_vec()
    } else {
        args
    };
    if is_window_copy || !cfg!(windows) {
        return run_window(&window_args);
    }
    // Check the options here, where an error can still be printed, then hand the window to a detached copy.
    if let Err(message) = parse_args(window_args.iter().cloned()) {
        eprintln!("{message}");
        return ExitCode::from(2);
    }
    start_detached_window(&window_args)
}

/// Starts the window in a copy of this program that has no console and is not tied to the terminal.
fn start_detached_window(args: &[OsString]) -> ExitCode {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("bridle: cannot find its own program: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut command = std::process::Command::new(exe);
    command
        .arg(GUI_FLAG)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    match command.spawn() {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bridle: cannot open the window: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_window(raw: &[OsString]) -> ExitCode {
    let args = match parse_args(raw.iter().cloned()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let (tx, rx) = mpsc::channel();
    let mut rx = Some(rx);
    let built = tauri::Builder::default()
        .manage(bridge::Bridge::new(args, tx))
        .setup(move |app| {
            let loaded = app.state::<bridge::Bridge>().load_args();
            if let Ok(workspace) = harness_cli::app::resolve_workspace(loaded.workspace.as_deref())
            {
                crew_cmd::recover_board(&workspace);
                bridge::remember_project(&workspace);
                bridge::set_window_title(app.handle(), &workspace);
            }
            if let Some(rx) = rx.take() {
                let handle = app.handle().clone();
                std::thread::Builder::new()
                    .name("bridle-pump".to_owned())
                    .spawn(move || bridge::pump(rx, &handle))?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bridge::get_snapshot,
            bridge::send_message,
            bridge::stop_turn,
            bridge::new_chat,
            bridge::refresh_snapshot,
            bridge::start_model,
            bridge::stop_model,
            bridge::run_doctor,
            bridge::run_health,
            bridge::setup_repository,
            bridge::setup_machine,
            bridge::suggest_machine_paths,
            bridge::reveal_machine_config,
            bridge::git_changes,
            bridge::write_workspace_file,
            bridge::git_show_head,
            bridge::create_entry,
            bridge::rename_entry,
            bridge::delete_entry,
            bridge::revert_file,
            bridge::read_image_preview,
            bridge::search_workspace,
            bridge::list_workspace_files,
            bridge::term_profiles,
            bridge::term_resize,
            bridge::term_start,
            bridge::term_write,
            bridge::term_stop,
            bridge::pick_image,
            bridge::attach_image,
            bridge::attach_image_bytes,
            bridge::get_network,
            bridge::set_network,
            bridge::test_network,
            bridge::list_commands,
            bridge::recent_projects,
            bridge::pick_folder,
            bridge::switch_workspace,
            bridge::git_file_diff,
            bridge::list_dir,
            bridge::read_workspace_file,
            ideas_cmd::list_ideas,
            ideas_cmd::add_idea,
            ideas_cmd::update_idea,
            ideas_cmd::delete_idea,
            ideas_cmd::move_idea,
            ideas_cmd::set_ideas_hold,
            ideas_cmd::take_idea,
            crew_cmd::get_crew,
            crew_cmd::crew_add_member,
            crew_cmd::crew_update_member,
            crew_cmd::crew_remove_member,
            crew_cmd::crew_set_headcount,
            crew_cmd::crew_reset,
            crew_cmd::resolve_tags,
            crew_cmd::create_tagged_issues,
            crew_cmd::get_board,
            crew_cmd::board_new_issue,
            crew_cmd::board_set_status,
            crew_cmd::board_assign,
            crew_cmd::board_comment,
            crew_cmd::board_export,
            crew_cmd::run_issue,
            crew_cmd::undo_issue,
        ])
        .build(tauri::generate_context!());
    let app = match built {
        Ok(app) => app,
        Err(error) => {
            eprintln!("bridle: cannot start the window: {error}");
            return ExitCode::FAILURE;
        }
    };
    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            handle.state::<bridge::Bridge>().shutdown();
        }
    });
    ExitCode::SUCCESS
}
