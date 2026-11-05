use super::*;

/// Hidden flag for the child that queries the terminal's image protocol.
pub(super) const PROBE_ARG: &str = "--probe-image-protocol";

pub(super) const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The child collects replies within this deadline; the parent bounds the process.
pub(super) const PROBE_QUERY_TIMEOUT: Duration = Duration::from_secs(1);

/// Child side: prints `<protocol> <font width> <font height>` to stderr; stdout is the terminal.
pub(super) fn probe_image_protocol() {
    use ratatui_image::picker::cap_parser::QueryStdioOptions;

    let tmux = Picker::halfblocks().tmux_detected();
    let mut options = QueryStdioOptions {
        timeout: PROBE_QUERY_TIMEOUT,
        ..QueryStdioOptions::default()
    };
    if ["WEZTERM_EXECUTABLE", "KONSOLE_VERSION"]
        .iter()
        .any(|name| std::env::var(name).is_ok_and(|value| !value.is_empty()))
    {
        options.blacklist_protocols = vec![
            ratatui_image::picker::ProtocolType::Kitty,
            ratatui_image::picker::ProtocolType::Sixel,
        ];
    }
    if crossterm::terminal::enable_raw_mode().is_err() {
        return;
    }
    let result = query_image_protocol(&std::io::stdin(), &mut std::io::stdout(), tmux, options);
    let _ = crossterm::terminal::disable_raw_mode();
    let Ok((protocol, font)) = result else { return };
    let Some(font) = font.or_else(|| {
        (!tmux)
            .then(crossterm::terminal::window_size)
            .and_then(Result::ok)
            .and_then(|window| {
                Some(ratatui_image::FontSize::new(
                    window
                        .width
                        .checked_div(window.columns)
                        .filter(|n| *n > 0)?,
                    window.height.checked_div(window.rows).filter(|n| *n > 0)?,
                ))
            })
    }) else {
        return;
    };
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(font);
    if let Some(protocol) = protocol {
        picker.set_protocol_type(protocol);
    }
    let kind = match picker.protocol_type() {
        ratatui_image::picker::ProtocolType::Kitty => "kitty",
        ratatui_image::picker::ProtocolType::Sixel => "sixel",
        ratatui_image::picker::ProtocolType::Iterm2 => "iterm2",
        ratatui_image::picker::ProtocolType::Halfblocks => "halfblocks",
    };
    eprintln!("{kind} {} {}", font.width, font.height);
}

pub(super) fn query_image_protocol(
    input: &impl std::os::fd::AsRawFd,
    output: &mut impl std::io::Write,
    tmux: bool,
    options: ratatui_image::picker::cap_parser::QueryStdioOptions,
) -> std::io::Result<(
    Option<ratatui_image::picker::ProtocolType>,
    Option<ratatui_image::FontSize>,
)> {
    use ratatui_image::picker::{
        ProtocolType,
        cap_parser::{Parser, Response},
    };

    let deadline = Instant::now() + options.timeout;
    let blacklist = options.blacklist_protocols.clone();
    output.write_all(Parser::query(tmux, options).as_bytes())?;
    output.flush()?;
    let mut parser = Parser::new();
    let (mut protocol, mut font) = (None, None);
    let mut remaining = 8192;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let mut fd = libc::pollfd {
            fd: input.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe {
            libc::poll(
                &mut fd,
                1,
                left.as_millis().clamp(1, i32::MAX as u128) as i32,
            )
        };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready == 0 {
            break;
        }
        let mut bytes = [0; 256];
        let count = unsafe {
            libc::read(
                input.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                remaining.min(256),
            )
        };
        if count < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        let count = count as usize;
        if count == 0 {
            break;
        }
        remaining -= count;
        for &byte in &bytes[..count] {
            for response in parser.push(char::from(byte)) {
                match response {
                    Response::Kitty if !blacklist.contains(&ProtocolType::Kitty) => {
                        protocol = Some(ProtocolType::Kitty)
                    }
                    Response::Sixel
                        if protocol.is_none() && !blacklist.contains(&ProtocolType::Sixel) =>
                    {
                        protocol = Some(ProtocolType::Sixel)
                    }
                    Response::CellSize(Some((width, height))) => {
                        font = Some(ratatui_image::FontSize::new(width, height))
                    }
                    _ => {}
                }
            }
        }
        if remaining == 0 || (protocol == Some(ProtocolType::Kitty) && font.is_some()) {
            break;
        }
    }
    Ok((protocol, font))
}

/// Parent side. Querying in a child means a terminal that never answers can't leave a
/// blocked stdin reader in redtab that swallows the first key.
pub(super) fn detect_image_picker() -> Picker {
    let report = image_protocol_report();
    let (native_tmux, font) = if terminal_output::inside_tmux() {
        tmux_graphics()
    } else {
        (false, None)
    };
    let Some((protocol, font)) = image_protocol_choice(report.as_deref(), native_tmux, font) else {
        return Picker::halfblocks();
    };
    protocol_picker(protocol, font)
}

pub(super) fn protocol_picker(
    protocol: ratatui_image::picker::ProtocolType,
    font: ratatui_image::FontSize,
) -> Picker {
    // `from_fontsize` retains tmux passthrough wrapping.
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(font);
    picker.set_protocol_type(protocol);
    picker
}

pub(super) fn image_protocol_report() -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe().ok()?;
    let mut child = Command::new(exe)
        .arg(PROBE_ARG)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut report = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut report);
    }
    Some(report)
}

pub(super) fn image_protocol_choice(
    report: Option<&str>,
    native_tmux: bool,
    tmux_font: Option<ratatui_image::FontSize>,
) -> Option<(ratatui_image::picker::ProtocolType, ratatui_image::FontSize)> {
    use ratatui_image::{FontSize, picker::ProtocolType};
    let reported = report.and_then(|report| {
        let mut parts = report.split_whitespace();
        let protocol = match parts.next()? {
            "kitty" => ProtocolType::Kitty,
            "sixel" => ProtocolType::Sixel,
            "iterm2" => ProtocolType::Iterm2,
            "halfblocks" => ProtocolType::Halfblocks,
            _ => return None,
        };
        let width = parts.next()?.parse::<u16>().ok().filter(|n| *n > 0)?;
        let height = parts.next()?.parse::<u16>().ok().filter(|n| *n > 0)?;
        parts
            .next()
            .is_none()
            .then_some((protocol, FontSize::new(width, height)))
    });
    match reported {
        Some((protocol, font)) if native_tmux && protocol != ProtocolType::Halfblocks => {
            Some((ProtocolType::Kitty, font))
        }
        Some((protocol, font)) if protocol != ProtocolType::Halfblocks => Some((protocol, font)),
        _ if native_tmux => tmux_font
            .filter(|font| font.width > 0 && font.height > 0)
            .map(|font| (ProtocolType::Kitty, font)),
        _ => None,
    }
}

/// In tmux the probe may get no kitty reply even when the outer terminal supports it.
/// Use kitty only if redtab draws straight into its pane (a nested terminal drops
/// passthrough). Clients must belong to a session containing this window.
pub(super) fn tmux_graphics() -> (bool, Option<ratatui_image::FontSize>) {
    tmux_window_clients().map_or((false, None), |(clients, sessions)| {
        let sessions: Vec<&str> = sessions.iter().map(String::as_str).collect();
        tmux_client_graphics(&clients, &sessions)
    })
}

/// `list-clients` output and the sessions containing this pane's window.
pub(super) fn tmux_window_clients() -> Option<(String, Vec<String>)> {
    let pane = terminal_output::TmuxPane::for_stdout()?;
    let window = pane.command_for_pane(&["display-message", "-p", "#{window_id}"])?;
    let windows = pane.command(&["list-windows", "-a", "-F", "#{window_id} #{session_id}"])?;
    let sessions = windows
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(w, _)| *w == window)
        .map(|(_, s)| s.to_string())
        .collect();
    let clients = pane.command(&[
        "list-clients",
        "-F",
        "#{session_id} #{client_termname} #{client_cell_width} #{client_cell_height}",
    ])?;
    Some((clients, sessions))
}

pub(super) fn tmux_client_graphics(
    clients: &str,
    sessions: &[&str],
) -> (bool, Option<ratatui_image::FontSize>) {
    if sessions.is_empty() {
        return (false, None);
    }
    let mut clients: Vec<_> = clients
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .collect();
    let on_window = |fields: &Vec<&str>| {
        fields
            .first()
            .is_some_and(|session| sessions.contains(session))
    };
    clients.retain(on_window);
    if clients.is_empty()
        || !clients.iter().all(|fields| {
            fields
                .get(1)
                .is_some_and(|name| name.contains("ghostty") || name.contains("kitty"))
        })
    {
        return (false, None);
    }
    let mut sizes = clients.iter().map(|fields| {
        let width = fields.get(2)?.parse::<u16>().ok().filter(|n| *n > 0)?;
        let height = fields.get(3)?.parse::<u16>().ok().filter(|n| *n > 0)?;
        Some((width, height))
    });
    let Some(first) = sizes.next().flatten() else {
        return (true, None);
    };
    let font = sizes
        .all(|size| size == Some(first))
        .then(|| ratatui_image::FontSize::new(first.0, first.1));
    (true, font)
}

#[cfg(test)]
mod tests;
