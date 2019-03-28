// Copyright 2014-2017 The Rpassword Developers
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[cfg(unix)]
extern crate libc;

use std::io::Write;

/// Sets all bytes of a String to 0
fn zero_memory(s: &mut String) {
    let vec = unsafe { s.as_mut_vec() };
    for el in vec.iter_mut() {
        *el = 0u8;
    }
}

/// Removes the \n from the read line
fn fixes_newline(password: &mut String) {
    // We may not have a newline, e.g. if user sent CTRL-D or if
    // this is not a TTY.

    if password.ends_with('\n') {
        // Remove the \n from the line if present
        password.pop();

        // Remove the \r from the line if present
        if password.ends_with('\r') {
            password.pop();
        }
    }
}

/// Reads a password from STDIN
pub fn read_password() -> ::std::io::Result<String> {
    read_password_with_reader(None::<::std::io::Empty>)
}

#[cfg(unix)]
mod unix {
    use libc::{c_int, isatty, tcgetattr, tcsetattr, TCSANOW, ECHO, ECHONL, STDIN_FILENO};
    use std::io::{self, BufRead, Write};
    use std::os::unix::io::AsRawFd;

    /// Turns a C function return into an IO Result
    fn io_result(ret: c_int) -> ::std::io::Result<()> {
        match ret {
            0 => Ok(()),
            _ => Err(::std::io::Error::last_os_error()),
        }
    }

    /// Reads a password from stdin
    pub fn read_password_from_stdin(open_tty: bool) -> ::std::io::Result<String> {
        let mut password = String::new();

        enum Source {
            Tty(io::BufReader<::std::fs::File>),
            Stdin(io::Stdin),
        }

        let (tty_fd, mut source) = if open_tty {
            let tty = ::std::fs::File::open("/dev/tty")?;
            (tty.as_raw_fd(), Source::Tty(io::BufReader::new(tty)))
        } else {
            (STDIN_FILENO, Source::Stdin(io::stdin()))
        };

        let input_is_tty = unsafe { isatty(tty_fd) } == 1;

        // When we ask for a password in a terminal, we'll want to hide the password as it is
        // typed by the user
        if input_is_tty {
            // Make two copies of the terminal settings. The first one will be modified
            // and the second one will act as a backup for when we want to set the
            // terminal back to its original state.
            let mut term = unsafe { ::std::mem::uninitialized() };
            let mut term_orig = unsafe { ::std::mem::uninitialized() };
            io_result(unsafe { tcgetattr(tty_fd, &mut term) })?;
            io_result(unsafe { tcgetattr(tty_fd, &mut term_orig) })?;

            // Hide the password. This is what makes this function useful.
            term.c_lflag &= !ECHO;

            // But don't hide the NL character when the user hits ENTER.
            term.c_lflag |= ECHONL;

            // Save the settings for now.
            io_result(unsafe { tcsetattr(tty_fd, TCSANOW, &term) })?;

            // Read the password.
            let input = match source {
                Source::Tty(ref mut tty) => tty.read_line(&mut password),
                Source::Stdin(ref mut stdin) => stdin.read_line(&mut password),
            };

            // Check the response.
            match input {
                Ok(_) => {}
                Err(err) => {
                    // Reset the terminal and quit.
                    io_result(unsafe { tcsetattr(tty_fd, TCSANOW, &term_orig) })?;

                    super::zero_memory(&mut password);
                    return Err(err);
                }
            };

            // Reset the terminal.
            match io_result(unsafe { tcsetattr(tty_fd, TCSANOW, &term_orig) }) {
                Ok(_) => {}
                Err(err) => {
                    super::zero_memory(&mut password);
                    return Err(err);
                }
            }
        } else {
            // If we don't have a TTY, the input was piped so we bypass
            // terminal hiding code
            let input = match source {
                Source::Tty(mut tty) => tty.read_line(&mut password),
                Source::Stdin(mut stdin) => stdin.read_line(&mut password),
            };

            match input {
                Ok(_) => {}
                Err(err) => {
                    super::zero_memory(&mut password);
                    return Err(err);
                }
            }
        }

        super::fixes_newline(&mut password);

        Ok(password)
    }

    /// Displays a prompt on the terminal
    pub fn display_on_tty(prompt: &str) -> ::std::io::Result<()> {
        let mut stream =
            ::std::fs::OpenOptions::new().write(true).open("/dev/tty")?;
        write!(stream, "{}", prompt)?;
        stream.flush()
    }
}

#[cfg(windows)]
mod windows {
    extern crate winapi;
    extern crate kernel32;
    use std::io::{self, BufRead, Write};
    use std::os::windows::io::{FromRawHandle, IntoRawHandle};
    use self::winapi::winnt::{
        GENERIC_READ, GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    use self::winapi::fileapi::OPEN_EXISTING;

    /// Reads a password from stdin
    pub fn read_password_from_stdin(open_tty: bool) -> ::std::io::Result<String> {
        let mut password = String::new();

        // Get the stdin handle
        let handle = if open_tty {
            unsafe {
                kernel32::CreateFileA(b"CONIN$\x00".as_ptr() as *const i8,
                                      GENERIC_READ | GENERIC_WRITE,
                                      FILE_SHARE_READ | FILE_SHARE_WRITE,
                                      ::std::ptr::null_mut(), OPEN_EXISTING, 0,
                                      ::std::ptr::null_mut())
            }
        } else {
            unsafe {
                kernel32::GetStdHandle(winapi::STD_INPUT_HANDLE)
            }
        };
        if handle == winapi::INVALID_HANDLE_VALUE {
            return Err(::std::io::Error::last_os_error());
        }

        // Get the old mode so we can reset back to it when we are done
        let mut mode = 0;
        if unsafe { kernel32::GetConsoleMode(handle, &mut mode as winapi::LPDWORD) } == 0 {
            return Err(::std::io::Error::last_os_error());
        }

        // We want to be able to read line by line, and we still want backspace to work
        let new_mode_flags = winapi::ENABLE_LINE_INPUT | winapi::ENABLE_PROCESSED_INPUT;
        if unsafe { kernel32::SetConsoleMode(handle, new_mode_flags) } == 0 {
            return Err(::std::io::Error::last_os_error());
        }

        // Read the password.
        let mut source = io::BufReader::new(unsafe {
            ::std::fs::File::from_raw_handle(handle)
        });
        let input = source.read_line(&mut password);
        let handle = source.into_inner().into_raw_handle();

        // Check the response.
        match input {
            Ok(_) => {}
            Err(err) => {
                super::zero_memory(&mut password);
                return Err(err);
            }
        };

        // Set the the mode back to normal
        if unsafe { kernel32::SetConsoleMode(handle, mode) } == 0 {
            return Err(::std::io::Error::last_os_error());
        }

        super::fixes_newline(&mut password);
        println!("\n");
        Ok(password)
    }

    /// Displays a prompt on the terminal
    pub fn display_on_tty(prompt: &str) -> ::std::io::Result<()> {
        let handle = unsafe {
            kernel32::CreateFileA(b"CONOUT$\x00".as_ptr() as *const i8,
                                  GENERIC_READ | GENERIC_WRITE,
                                  FILE_SHARE_READ | FILE_SHARE_WRITE,
                                  ::std::ptr::null_mut(), OPEN_EXISTING, 0,
                                  ::std::ptr::null_mut())
        };
        if handle == winapi::INVALID_HANDLE_VALUE {
            return Err(::std::io::Error::last_os_error());
        }

        let mut stream = unsafe {
            ::std::fs::File::from_raw_handle(handle)
        };

        write!(stream, "{}", prompt)?;
        stream.flush()
    }
}

#[cfg(unix)]
use unix::{read_password_from_stdin, display_on_tty};
#[cfg(windows)]
use windows::{read_password_from_stdin, display_on_tty};

/// Reads a password from anything that implements BufRead
pub fn read_password_with_reader<T>(source: Option<T>) -> ::std::io::Result<String>
    where T: ::std::io::BufRead {
    match source {
        Some(mut reader) => {
            let mut password = String::new();
            if let Err(err) = reader.read_line(&mut password) {
                zero_memory(&mut password);
                Err(err)
            } else {
                fixes_newline(&mut password);
                Ok(password)
            }
        },
        None => read_password_from_stdin(false),
    }
}

/// Reads a password from the terminal
pub fn read_password_from_tty(prompt: Option<&str>)
                              -> ::std::io::Result<String> {
    if let Some(prompt) = prompt {
        display_on_tty(prompt)?;
    }
    read_password_from_stdin(true)
}

/// Prompts for a password on STDOUT and reads it from STDIN
pub fn prompt_password_stdout(prompt: &str) -> std::io::Result<String> {
    let mut stdout = std::io::stdout();

    write!(stdout, "{}", prompt)?;
    stdout.flush()?;
    read_password()
}

/// Prompts for a password on STDERR and reads it from STDIN
pub fn prompt_password_stderr(prompt: &str) -> std::io::Result<String> {
    let mut stderr = std::io::stderr();

    write!(stderr, "{}", prompt)?;
    stderr.flush()?;
    read_password()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    fn mock_input_crlf() -> Cursor<&'static [u8]> {
        Cursor::new(&b"A mocked response.\r\n"[..])
    }

    fn mock_input_lf() -> Cursor<&'static [u8]> {
        Cursor::new(&b"A mocked response.\n"[..])
    }

    #[test]
    fn can_read_from_redirected_input() {
        let response = ::read_password_with_reader(Some(mock_input_crlf())).unwrap();
        assert_eq!(response, "A mocked response.");
        let response = ::read_password_with_reader(Some(mock_input_lf())).unwrap();
        assert_eq!(response, "A mocked response.");
    }
}
