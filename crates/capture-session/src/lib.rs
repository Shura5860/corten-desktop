//! End-to-end screen-capture orchestration.
//!
//! Portal negotiation remains in corten-services, PipeWire remains in
//! corten-capture, and this layer owns their lifetime and shutdown ordering.

use std::sync::mpsc::Receiver;

use corten_capture::{CaptureConfig, CaptureHandle, FrameConsumer, FramePump, VideoFrame};
use corten_services::portal::ScreenCastSession;

#[derive(Debug, thiserror::Error)]
pub enum CaptureSessionError {
    #[error("portal session has no PipeWire streams")]
    NoStreams,
    #[error("capture could not start: {0}")]
    Capture(String),
    #[error("a frame consumer is already attached")]
    ConsumerAlreadyAttached,
    #[error("stream index {0} is unavailable")]
    StreamUnavailable(usize),
    #[error("frame consumer failed: {0}")]
    Consumer(String),
    #[error("portal session could not close: {0}")]
    PortalClose(String),
}

pub struct CaptureSession {
    portal: Option<ScreenCastSession>,
    captures: Vec<CaptureHandle>,
    frames: Vec<Option<Receiver<VideoFrame>>>,
    pumps: Vec<Option<FramePump>>,
}

impl CaptureSession {
    pub async fn start(monitor_only: bool) -> Result<Self, CaptureSessionError> {
        Self::start_with_options(monitor_only, false, 0).await
    }

    pub async fn start_with_options(
        monitor_only: bool,
        multiple: bool,
        stream_index: usize,
    ) -> Result<Self, CaptureSessionError> {
        let portal =
            corten_services::portal::start_screencast_session_with_options(monitor_only, multiple)
                .await
                .map_err(CaptureSessionError::Capture)?;
        let node_ids = if multiple {
            portal.node_ids().to_vec()
        } else {
            portal
                .node_ids()
                .get(stream_index)
                .copied()
                .into_iter()
                .collect()
        };
        if node_ids.is_empty() {
            let _ = portal.close().await;
            return Err(CaptureSessionError::NoStreams);
        }
        let mut captures = Vec::with_capacity(node_ids.len());
        let mut frames = Vec::with_capacity(node_ids.len());
        for node_id in node_ids {
            match corten_capture::start(CaptureConfig::new(node_id)) {
                Ok((capture, receiver)) => {
                    captures.push(capture);
                    frames.push(Some(receiver));
                }
                Err(error) => {
                    for capture in captures {
                        capture.stop();
                    }
                    let _ = portal.close().await;
                    return Err(CaptureSessionError::Capture(error.to_string()));
                }
            }
        }
        let pumps = (0..captures.len()).map(|_| None).collect();
        Ok(Self {
            portal: Some(portal),
            captures,
            frames,
            pumps,
        })
    }

    pub async fn start_all(monitor_only: bool) -> Result<Self, CaptureSessionError> {
        Self::start_with_options(monitor_only, true, 0).await
    }

    pub fn stream_count(&self) -> usize {
        self.captures.len()
    }

    pub fn attach_consumer<C: FrameConsumer>(
        &mut self,
        stream_index: usize,
        consumer: C,
    ) -> Result<(), CaptureSessionError> {
        let pump = self
            .pumps
            .get_mut(stream_index)
            .ok_or(CaptureSessionError::StreamUnavailable(stream_index))?;
        if pump.is_some() {
            return Err(CaptureSessionError::ConsumerAlreadyAttached);
        }
        let frames = self
            .frames
            .get_mut(stream_index)
            .and_then(Option::take)
            .ok_or(CaptureSessionError::StreamUnavailable(stream_index))?;
        *pump = Some(FramePump::start(frames, consumer));
        Ok(())
    }

    pub async fn stop(mut self) -> Result<(), CaptureSessionError> {
        let mut first_error = None;
        for pump in self.pumps.into_iter().flatten() {
            if let Err(error) = pump.stop() {
                if first_error.is_none() {
                    first_error = Some(CaptureSessionError::Consumer(error));
                }
            }
        }
        self.frames.clear();
        for capture in self.captures {
            capture.stop();
        }
        if let Some(portal) = self.portal.take() {
            if let Err(error) = portal.close().await {
                if first_error.is_none() {
                    first_error = Some(CaptureSessionError::PortalClose(error));
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}
