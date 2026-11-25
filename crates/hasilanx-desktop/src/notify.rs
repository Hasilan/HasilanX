use std::process::{Command, Stdio};

pub fn send_notification(title: &str, body: &str, with_sound: bool) {
    let title = title.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new()
            .appname("HasilanX")
            .summary(&title)
            .body(&body)
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show();

        if with_sound {
            play_chime();
        }
    });
}

fn play_chime() {
    #[cfg(target_os = "linux")]
    {
        // Try freedesktop sound event via canberra-gtk-play or paplay silently
        let played = Command::new("canberra-gtk-play")
            .args(["-i", "message-new-instant"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !played {
            let _ = Command::new("paplay")
                .arg("/usr/share/sounds/freedesktop/stereo/message.oga")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("afplay")
            .arg("/System/Library/Sounds/Glass.aiff")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
