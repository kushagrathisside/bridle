//! Helpers for the programs the harness starts on the person's behalf.

use std::process::Command;

/// Keeps the program from opening a console window of its own on Windows. The desktop app has no
/// console, so every `git`, shell or test command it starts would otherwise flash one. Does nothing
/// on other systems, and does not change what the program reads or writes.
#[cfg(windows)]
pub fn hide_console(command: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    /// `CREATE_NO_WINDOW`.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW)
}

#[cfg(not(windows))]
pub fn hide_console(command: &mut Command) -> &mut Command {
    command
}
