//! Screen-capture transport kept independent from the desktop shell.
//!
//! Portal negotiation belongs in `corten-services`; this crate only consumes
//! the PipeWire node id returned by the portal. Frames are copied into a
//! bounded channel so a slow encoder or compositor can never stall PipeWire's
//! realtime callback.

use std::io::{self, Write};
#[cfg(feature = "pipewire")]
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::JoinHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureConfig {
    pub node_id: u32,
    pub channel_capacity: usize,
}

impl CaptureConfig {
    pub fn new(node_id: u32) -> Self {
        Self {
            node_id,
            channel_capacity: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub stride: i32,
    pub format: PixelFormat,
    pub timestamp_ns: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgrx,
    Rgbx,
    Rgba,
    Unknown(u32),
}

pub trait FrameConsumer: Send + 'static {
    fn consume(&mut self, frame: VideoFrame) -> Result<(), String>;
}

/// Lossless diagnostic sink used to validate capture before adding a codec.
/// The stream starts with `CORTENRF\0` and version 1, followed by length-
/// prefixed frame records. It is not a playback format.
pub struct RawFrameRecorder<W> {
    writer: W,
}

impl RawFrameRecorder<io::BufWriter<std::fs::File>> {
    pub fn open(path: impl AsRef<std::path::Path>) -> io::Result<Self> {
        Self::new(io::BufWriter::new(std::fs::File::create(path)?))
    }
}

impl<W: Write> RawFrameRecorder<W> {
    pub fn new(mut writer: W) -> io::Result<Self> {
        writer.write_all(b"CORTENRF\0")?;
        writer.write_all(&1u32.to_le_bytes())?;
        Ok(Self { writer })
    }

    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl<W: Write + Send + 'static> FrameConsumer for RawFrameRecorder<W> {
    fn consume(&mut self, frame: VideoFrame) -> Result<(), String> {
        self.writer
            .write_all(b"FRM0")
            .and_then(|_| self.writer.write_all(&frame.width.to_le_bytes()))
            .and_then(|_| self.writer.write_all(&frame.height.to_le_bytes()))
            .and_then(|_| self.writer.write_all(&frame.stride.to_le_bytes()))
            .and_then(|_| {
                self.writer
                    .write_all(&pixel_format_id(frame.format).to_le_bytes())
            })
            .and_then(|_| {
                self.writer
                    .write_all(&frame.timestamp_ns.unwrap_or(-1).to_le_bytes())
            })
            .and_then(|_| {
                self.writer
                    .write_all(&(frame.data.len() as u64).to_le_bytes())
            })
            .and_then(|_| self.writer.write_all(&frame.data))
            .map_err(|error| error.to_string())
    }
}

#[cfg(feature = "ffmpeg")]
pub struct FfmpegEncoder {
    child: std::process::Child,
    input: Option<std::process::ChildStdin>,
    width: u32,
    height: u32,
    conversion_row: Vec<u8>,
}

#[cfg(feature = "ffmpeg")]
impl FfmpegEncoder {
    /// Starts a native FFmpeg process without invoking a shell.
    pub fn open(
        path: impl AsRef<std::path::Path>,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 || fps == 0 {
            return Err("encoder dimensions and fps must be non-zero".into());
        }
        let size = format!("{width}x{height}");
        let rate = fps.to_string();
        let mut child = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "bgr0",
                "-s",
                &size,
                "-r",
                &rate,
                "-i",
                "pipe:0",
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(path.as_ref())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|error| format!("could not start ffmpeg: {error}"))?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| "ffmpeg stdin was not available".to_string())?;
        Ok(Self {
            child,
            input: Some(input),
            width,
            height,
            conversion_row: vec![0; width as usize * 4],
        })
    }
}

#[cfg(feature = "ffmpeg")]
impl FrameConsumer for FfmpegEncoder {
    fn consume(&mut self, frame: VideoFrame) -> Result<(), String> {
        if frame.width != self.width || frame.height != self.height {
            return Err("frame dimensions changed during encoding".into());
        }
        if !matches!(
            frame.format,
            PixelFormat::Bgrx | PixelFormat::Rgbx | PixelFormat::Rgba
        ) {
            return Err("ffmpeg encoder received an unsupported pixel format".into());
        }
        let row_bytes = frame.width as usize * 4;
        let stride = frame.stride.unsigned_abs() as usize;
        if stride < row_bytes || frame.data.len() < stride * frame.height as usize {
            return Err("frame stride or buffer length is invalid".into());
        }
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| "ffmpeg input is closed".to_string())?;
        for row in 0..frame.height as usize {
            let start = row * stride;
            let source = &frame.data[start..start + row_bytes];
            if frame.format == PixelFormat::Bgrx {
                input
                    .write_all(source)
                    .map_err(|error| format!("ffmpeg rejected a video frame: {error}"))?;
            } else {
                for pixel_index in 0..frame.width as usize {
                    let source_index = pixel_index * 4;
                    let destination_index = source_index;
                    self.conversion_row[destination_index] = source[source_index + 2];
                    self.conversion_row[destination_index + 1] = source[source_index + 1];
                    self.conversion_row[destination_index + 2] = source[source_index];
                    self.conversion_row[destination_index + 3] = 0;
                }
                input
                    .write_all(&self.conversion_row)
                    .map_err(|error| format!("ffmpeg rejected a video frame: {error}"))?;
            }
        }
        Ok(())
    }
}

#[cfg(feature = "ffmpeg")]
impl Drop for FfmpegEncoder {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.wait();
    }
}

#[cfg(feature = "ffmpeg")]
pub struct FfmpegConsumer {
    path: std::path::PathBuf,
    fps: u32,
    encoder: Option<FfmpegEncoder>,
}

#[cfg(feature = "ffmpeg")]
impl FfmpegConsumer {
    pub fn new(path: impl Into<std::path::PathBuf>, fps: u32) -> Result<Self, String> {
        if fps == 0 {
            return Err("encoder fps must be non-zero".into());
        }
        Ok(Self {
            path: path.into(),
            fps,
            encoder: None,
        })
    }
}

#[cfg(feature = "ffmpeg")]
impl FrameConsumer for FfmpegConsumer {
    fn consume(&mut self, frame: VideoFrame) -> Result<(), String> {
        if self.encoder.is_none() {
            self.encoder = Some(FfmpegEncoder::open(
                &self.path,
                frame.width,
                frame.height,
                self.fps,
            )?);
        }
        self.encoder
            .as_mut()
            .expect("encoder was initialized")
            .consume(frame)
    }
}

fn pixel_format_id(format: PixelFormat) -> u32 {
    match format {
        PixelFormat::Bgrx => 1,
        PixelFormat::Rgbx => 2,
        PixelFormat::Rgba => 3,
        PixelFormat::Unknown(value) => value,
    }
}

pub struct FramePump {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<JoinHandle<Result<(), String>>>,
}

impl FramePump {
    pub fn start<C: FrameConsumer>(receiver: Receiver<VideoFrame>, mut consumer: C) -> Self {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = std::sync::Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("corten-frame-consumer".into())
            .spawn(move || {
                while !thread_stop.load(std::sync::atomic::Ordering::Acquire) {
                    match receiver.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok(frame) => consumer.consume(frame)?,
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                Ok(())
            })
            .expect("frame consumer thread could not be spawned");
        Self {
            stop,
            thread: Some(thread),
        }
    }

    pub fn stop(mut self) -> Result<(), String> {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.thread
            .take()
            .expect("frame consumer thread already joined")
            .join()
            .map_err(|_| "frame consumer thread panicked".to_string())?
    }
}

impl Drop for FramePump {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("PipeWire capture support is not enabled")]
    FeatureDisabled,
    #[error("invalid frame channel capacity")]
    InvalidChannelCapacity,
    #[cfg(feature = "pipewire")]
    #[error("PipeWire error: {0}")]
    PipeWire(#[from] pipewire::Error),
    #[error("capture thread could not be started: {0}")]
    Thread(#[from] std::io::Error),
}

pub struct CaptureHandle {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl CaptureHandle {
    pub fn stop(mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn start(config: CaptureConfig) -> Result<(CaptureHandle, Receiver<VideoFrame>), CaptureError> {
    if config.channel_capacity == 0 {
        return Err(CaptureError::InvalidChannelCapacity);
    }

    #[cfg(not(feature = "pipewire"))]
    {
        let _ = config;
        Err(CaptureError::FeatureDisabled)
    }

    #[cfg(feature = "pipewire")]
    backend::start(config)
}

#[cfg(feature = "pipewire")]
mod backend {
    use super::*;
    use pipewire as pw;
    use pw::{properties::properties, spa};
    use spa::pod::Pod;
    use std::time::Duration;

    struct UserData {
        format: spa::param::video::VideoInfoRaw,
        sender: SyncSender<VideoFrame>,
    }

    pub(super) fn start(
        config: CaptureConfig,
    ) -> Result<(CaptureHandle, Receiver<VideoFrame>), CaptureError> {
        let (sender, receiver) = mpsc::sync_channel(config.channel_capacity);
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = std::sync::Arc::clone(&stop);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);

        let thread = std::thread::Builder::new()
            .name("corten-pipewire-capture".into())
            .spawn(move || {
                let result = run(config, sender, thread_stop, ready_sender.clone());
                if let Err(error) = result {
                    let _ = ready_sender.send(Err(error));
                }
            })?;

        match ready_receiver.recv() {
            Ok(Ok(())) => Ok((
                CaptureHandle {
                    stop,
                    thread: Some(thread),
                },
                receiver,
            )),
            Ok(Err(error)) => {
                stop.store(true, std::sync::atomic::Ordering::Release);
                let _ = thread.join();
                Err(error)
            }
            Err(error) => {
                stop.store(true, std::sync::atomic::Ordering::Release);
                let _ = thread.join();
                Err(CaptureError::Thread(error.into()))
            }
        }
    }

    fn run(
        config: CaptureConfig,
        sender: SyncSender<VideoFrame>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        ready_sender: SyncSender<Result<(), CaptureError>>,
    ) -> Result<(), CaptureError> {
        pw::init();
        let mainloop = pw::main_loop::MainLoop::new(None)?;
        let context = pw::context::Context::new(&mainloop)?;
        let core = context.connect(None)?;
        let stream = pw::stream::Stream::new(
            &core,
            "corten-screen-capture",
            properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
            },
        )?;

        let user_data = UserData {
            format: Default::default(),
            sender,
        };
        let _listener = stream
            .add_local_listener_with_user_data(user_data)
            .param_changed(|_, data, id, param| {
                let Some(param) = param else { return };
                if id != pw::spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Ok((media_type, media_subtype)) =
                    pw::spa::param::format_utils::parse_format(param)
                else {
                    return;
                };
                if media_type != pw::spa::param::format::MediaType::Video
                    || media_subtype != pw::spa::param::format::MediaSubtype::Raw
                {
                    return;
                }
                let _ = data.format.parse(param);
            })
            .process(|stream, data| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let datas = buffer.datas_mut();
                let Some(media) = datas.first_mut() else {
                    return;
                };
                let Some(bytes) = media.data() else { return };
                let offset = media.chunk().offset() as usize;
                let size = media.chunk().size() as usize;
                let Some(end) = offset.checked_add(size) else {
                    return;
                };
                if end > bytes.len() {
                    return;
                }
                let size = data.format.size();
                let frame = VideoFrame {
                    data: bytes[offset..end].to_vec(),
                    width: size.width,
                    height: size.height,
                    stride: media.chunk().stride(),
                    format: match data.format.format() {
                        spa::param::video::VideoFormat::BGRx => PixelFormat::Bgrx,
                        spa::param::video::VideoFormat::RGBx => PixelFormat::Rgbx,
                        spa::param::video::VideoFormat::RGBA => PixelFormat::Rgba,
                        format => PixelFormat::Unknown(format.as_raw()),
                    },
                    timestamp_ns: None,
                };
                match data.sender.try_send(frame) {
                    Ok(()) | Err(TrySendError::Disconnected(_)) => {}
                    Err(TrySendError::Full(_)) => {}
                }
            })
            .register()?;

        let obj = pw::spa::pod::object!(
            pw::spa::utils::SpaTypes::ObjectParamFormat,
            pw::spa::param::ParamType::EnumFormat,
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::MediaType,
                Id,
                pw::spa::param::format::MediaType::Video
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::MediaSubtype,
                Id,
                pw::spa::param::format::MediaSubtype::Raw
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::VideoFormat,
                Choice,
                Enum,
                Id,
                pw::spa::param::video::VideoFormat::BGRx,
                pw::spa::param::video::VideoFormat::RGBx,
                pw::spa::param::video::VideoFormat::RGBA
            )
        );
        let values = pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pw::spa::pod::Value::Object(obj),
        )
        .map_err(|_| {
            CaptureError::Thread(std::io::Error::other(
                "PipeWire format serialization failed",
            ))
        })?
        .0
        .into_inner();
        let mut params = [Pod::from_bytes(&values).ok_or_else(|| {
            CaptureError::Thread(std::io::Error::other("invalid PipeWire format pod"))
        })?];
        stream.connect(
            spa::utils::Direction::Input,
            Some(config.node_id),
            pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )?;

        let _ = ready_sender.send(Ok(()));
        while !stop.load(std::sync::atomic::Ordering::Acquire) {
            mainloop.loop_().iterate(Duration::from_millis(100));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_has_small_bounded_default() {
        assert_eq!(CaptureConfig::new(42).channel_capacity, 2);
    }

    struct CountingConsumer(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    impl FrameConsumer for CountingConsumer {
        fn consume(&mut self, _frame: VideoFrame) -> Result<(), String> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
    }

    #[test]
    fn frame_pump_consumes_without_blocking_producer() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(2);
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let pump = FramePump::start(receiver, CountingConsumer(std::sync::Arc::clone(&count)));
        sender
            .send(VideoFrame {
                data: vec![0; 4],
                width: 1,
                height: 1,
                stride: 4,
                format: PixelFormat::Bgrx,
                timestamp_ns: None,
            })
            .expect("consumer should be alive");
        drop(sender);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while count.load(std::sync::atomic::Ordering::Relaxed) == 0
            && std::time::Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        pump.stop().expect("consumer should stop cleanly");
        assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn raw_recorder_writes_header_and_frame() {
        let mut recorder = RawFrameRecorder::new(std::io::Cursor::new(Vec::new()))
            .expect("header should be writable");
        recorder
            .consume(VideoFrame {
                data: vec![1, 2, 3, 4],
                width: 1,
                height: 1,
                stride: 4,
                format: PixelFormat::Bgrx,
                timestamp_ns: Some(7),
            })
            .expect("frame should be writable");
        let output = recorder.into_inner().into_inner();
        assert_eq!(&output[..9], b"CORTENRF\0");
        assert_eq!(&output[13..17], b"FRM0");
    }

    #[cfg(not(feature = "pipewire"))]
    #[test]
    fn disabled_backend_is_explicit() {
        assert!(matches!(
            start(CaptureConfig::new(42)),
            Err(CaptureError::FeatureDisabled)
        ));
    }
}
