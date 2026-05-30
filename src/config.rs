use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "kitweb",
    version,
    about = "A terminal browser proxy using the Kitty graphics protocol"
)]
pub struct Config {
    /// URL to open
    #[arg(required = true)]
    pub url: String,

    /// Browser viewport width in pixels
    #[arg(long, default_value_t = 1680)]
    pub width: u32,

    /// Browser viewport height in pixels
    #[arg(long, default_value_t = 1260)]
    pub height: u32,

    /// Capture frame rate
    #[arg(long, default_value_t = 30)]
    pub fps: u32,

    /// Xvfb display number
    #[arg(long, default_value_t = 99)]
    pub display: u8,

    /// Disable browser audio capture/playback
    #[arg(long)]
    pub no_audio: bool,

    /// PulseAudio/PipeWire server used for Chromium audio capture
    #[arg(long)]
    pub audio_capture_server: Option<String>,
}
