#[cfg(feature = "ffmpeg")]
use std::sync::Arc;

#[cfg(feature = "ffmpeg")]
use tokio::sync::Mutex;

#[cfg(feature = "ffmpeg")]
struct CaptureServer {
    session: Arc<Mutex<Option<corten_capture_session::CaptureSession>>>,
}

#[cfg(feature = "ffmpeg")]
#[zbus::interface(name = "org.corten.Capture")]
impl CaptureServer {
    async fn start_recording(
        &self,
        output: &str,
        fps: u32,
        monitor_only: bool,
    ) -> zbus::fdo::Result<()> {
        let mut state = self.session.lock().await;
        if state.is_some() {
            return Err(zbus::fdo::Error::Failed(
                "recording is already active".into(),
            ));
        }
        let mut session = corten_capture_session::CaptureSession::start(monitor_only)
            .await
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        let consumer = corten_capture::FfmpegConsumer::new(output, fps)
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        session
            .attach_consumer(0, consumer)
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        *state = Some(session);
        Ok(())
    }

    async fn stop_recording(&self) -> zbus::fdo::Result<()> {
        let session = self.session.lock().await.take();
        if let Some(session) = session {
            session
                .stop()
                .await
                .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        }
        Ok(())
    }

    async fn get_recording(&self) -> bool {
        self.session.lock().await.is_some()
    }
}

#[cfg(feature = "ffmpeg")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = CaptureServer {
        session: Arc::new(Mutex::new(None)),
    };
    let _connection = zbus::connection::Builder::session()?
        .name("org.corten.Capture")?
        .serve_at("/org/corten/Capture", server)?
        .build()
        .await?;
    std::future::pending::<()>().await;
    Ok(())
}

#[cfg(not(feature = "ffmpeg"))]
fn main() {
    eprintln!("corten-capture-service requires the ffmpeg feature");
    std::process::exit(2);
}
