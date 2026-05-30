use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

pub struct BrowserSession {
    pub display: u8,
    pub width: u32,
    pub height: u32,
    xvfb: Child,
    chrome: Option<Child>,
}

pub struct BrowserAudioEnv<'a> {
    pub pulse_sink: &'a str,
    pub pulse_server: Option<&'a str>,
}

struct BrowserCommand {
    path: String,
    is_snap: bool,
}

impl BrowserSession {
    pub fn new(display: u8, width: u32, height: u32) -> Result<Self, Box<dyn std::error::Error>> {
        let display = find_free_display(display);
        let mut xvfb_cmd = Command::new("Xvfb");
        xvfb_cmd.args([
            &format!(":{}", display),
            "-screen",
            "0",
            &format!("{}x{}x24", width, height),
            "-ac",
        ]);
        configure_child_output(&mut xvfb_cmd);

        let mut xvfb = xvfb_cmd
            .spawn()
            .map_err(|e| format!("Failed to start Xvfb: {}. Is xvfb installed?", e))?;

        // Give Xvfb time to initialize
        thread::sleep(Duration::from_millis(500));
        if let Some(status) = xvfb.try_wait()? {
            return Err(format!("Xvfb exited immediately with status {}", status).into());
        }

        Ok(Self {
            display,
            width,
            height,
            xvfb,
            chrome: None,
        })
    }

    pub fn open_url(
        &mut self,
        url: &str,
        audio_env: Option<BrowserAudioEnv<'_>>,
        mute_audio: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Kill previous Chrome if running
        if let Some(mut prev) = self.chrome.take() {
            let _ = prev.kill();
        }

        let display_str = format!(":{}", self.display);

        let browser = find_browser()
            .ok_or("No Chrome/Chromium browser found. Install google-chrome or chromium.")?;

        let mut browser_cmd = Command::new(&browser.path);
        browser_cmd
            .env("DISPLAY", &display_str)
            .env("GTK_IM_MODULE", "xim")
            .env("NO_AT_BRIDGE", "1")
            .env("QT_IM_MODULE", "xim")
            .env("XMODIFIERS", "@im=none")
            .env_remove("WAYLAND_DISPLAY")
            .args(CHROME_ARGS)
            .arg(format!("--window-size={},{}", self.width, self.height))
            .arg(url);
        if let Some(audio_env) = audio_env {
            configure_browser_audio(&mut browser_cmd, &browser, audio_env);
        }
        if mute_audio {
            browser_cmd.arg("--mute-audio");
        }
        configure_child_output(&mut browser_cmd);

        let mut child = browser_cmd
            .spawn()
            .map_err(|e| format!("Failed to start browser: {}", e))?;

        // Give the browser time to open
        thread::sleep(Duration::from_millis(1500));
        if let Some(status) = child.try_wait()? {
            return Err(format!("Browser exited immediately with status {}", status).into());
        }

        self.chrome = Some(child);
        Ok(())
    }

    pub fn navigate(&self, url: &str) -> Result<(), Box<dyn std::error::Error>> {
        let display_str = format!(":{}", self.display);
        // Focus address bar
        Command::new("xdotool")
            .env("DISPLAY", &display_str)
            .args(["key", "--clearmodifiers", "ctrl+l"])
            .output()
            .map_err(|e| format!("xdotool not found: {}. Install xdotool.", e))?;

        thread::sleep(Duration::from_millis(100));

        // Type the URL
        Command::new("xdotool")
            .env("DISPLAY", &display_str)
            .args(["type", "--clearmodifiers", url])
            .output()?;

        thread::sleep(Duration::from_millis(50));

        // Press Enter
        Command::new("xdotool")
            .env("DISPLAY", &display_str)
            .args(["key", "--clearmodifiers", "Return"])
            .output()?;

        Ok(())
    }

    pub fn reload(&self) {
        let _ = Command::new("xdotool")
            .env("DISPLAY", self.display_str())
            .args(["key", "--clearmodifiers", "F5"])
            .output();
    }

    pub fn send_key(&self, key: &str) {
        let _ = Command::new("xdotool")
            .env("DISPLAY", self.display_str())
            .args(["key", "--clearmodifiers", key])
            .output();
    }

    /// Simulate a mouse scroll: direction 1 = up (button 4), -1 = down (button 5)
    pub fn scroll(&self, direction: i8) {
        let button = if direction > 0 { "4" } else { "5" };
        let _ = Command::new("xdotool")
            .env("DISPLAY", self.display_str())
            .args(["click", button])
            .output();
    }

    /// Simulate a horizontal mouse scroll: direction 1 = left (button 6), -1 = right (button 7)
    pub fn scroll_horizontal(&self, direction: i8) {
        let button = if direction > 0 { "6" } else { "7" };
        let _ = Command::new("xdotool")
            .env("DISPLAY", self.display_str())
            .args(["click", button])
            .output();
    }

    pub fn click_at(&self, button: u8, x: u32, y: u32) -> Result<(), Box<dyn std::error::Error>> {
        Command::new("xdotool")
            .env("DISPLAY", self.display_str())
            .arg("mousemove")
            .arg("--sync")
            .arg(x.to_string())
            .arg(y.to_string())
            .arg("click")
            .arg(button.to_string())
            .output()
            .map_err(|e| format!("xdotool not found: {}. Install xdotool.", e))?;

        Ok(())
    }

    pub fn type_text_and_enter(&self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        Command::new("xdotool")
            .env("DISPLAY", self.display_str())
            .args(["type", "--clearmodifiers", text])
            .output()
            .map_err(|e| format!("xdotool not found: {}. Install xdotool.", e))?;

        thread::sleep(Duration::from_millis(50));

        Command::new("xdotool")
            .env("DISPLAY", self.display_str())
            .args(["key", "--clearmodifiers", "Return"])
            .output()?;

        Ok(())
    }

    fn display_str(&self) -> String {
        format!(":{}", self.display)
    }
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        if let Some(mut chrome) = self.chrome.take() {
            let _ = chrome.kill();
        }
        let _ = self.xvfb.kill();
    }
}

const CHROME_ARGS: &[&str] = &[
    "--no-sandbox",
    "--no-first-run",
    "--disable-extensions",
    "--disable-gpu",
    "--disable-gpu-compositing",
    "--disable-accelerated-video-decode",
    "--disable-accelerated-video-encode",
    "--disable-dev-shm-usage",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-default-apps",
    "--disable-sync",
    "--disable-features=UsePortal,WebRtcUsePipeWireCapturer",
    "--force-device-scale-factor=1",
    "--log-level=3",
    "--no-default-browser-check",
    "--ozone-platform=x11",
    "--password-store=basic",
    "--use-gl=disabled",
];

fn configure_child_output(command: &mut Command) {
    if std::env::var_os("KITWEB_CHILD_LOGS").is_none() {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
}

fn find_free_display(preferred: u8) -> u8 {
    for n in preferred..200 {
        let lock = format!("/tmp/.X{}-lock", n);
        if !std::path::Path::new(&lock).exists() {
            return n;
        }
    }
    preferred
}

fn find_browser() -> Option<BrowserCommand> {
    ["google-chrome", "chromium-browser", "chromium"]
        .into_iter()
        .find_map(|name| {
            let path = which_path(name)?;
            let is_snap = is_snap_browser(name, &path);
            Some(BrowserCommand { path, is_snap })
        })
}

fn configure_browser_audio(
    command: &mut Command,
    browser: &BrowserCommand,
    audio_env: BrowserAudioEnv<'_>,
) {
    command.env("PULSE_SINK", audio_env.pulse_sink);

    if let Some(pulse_server) = audio_env.pulse_server {
        if browser.is_snap && is_unix_pulse_server(pulse_server) {
            // Snap Chromium already runs behind snap's PulseAudio mediation. Passing a
            // raw host unix socket can make it fall back to ALSA, while PULSE_SINK
            // still selects the kitweb null sink on the mediated local Pulse server.
            command.env_remove("PULSE_SERVER");
        } else {
            command.env("PULSE_SERVER", pulse_server);
        }
    }
}

fn which_path(name: &str) -> Option<String> {
    let output = Command::new("which").arg(name).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    (!path.is_empty()).then_some(path)
}

fn is_snap_browser(name: &str, path: &str) -> bool {
    if path.contains("/snap/") || path == "/snap/bin/chromium" {
        return true;
    }

    (name == "chromium" || name == "chromium-browser") && snap_chromium_installed()
}

fn snap_chromium_installed() -> bool {
    Command::new("snap")
        .args(["list", "chromium"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn is_unix_pulse_server(server: &str) -> bool {
    server.starts_with("unix:")
}
