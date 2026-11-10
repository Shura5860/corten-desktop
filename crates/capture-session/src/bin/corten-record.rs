#[cfg(feature = "ffmpeg")]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args_os().skip(1);
    let mut monitor_only = true;
    let mut all_streams = false;
    let mut stream_index = 0usize;
    let mut fps = 30u32;
    let mut path = None;
    while let Some(argument) = args.next() {
        match argument.to_string_lossy().as_ref() {
            "--window" => monitor_only = false,
            "--all" => all_streams = true,
            "--stream" => {
                stream_index = args
                    .next()
                    .and_then(|value| value.to_str()?.parse().ok())
                    .unwrap_or_else(|| {
                        eprintln!("--stream requires a non-negative index");
                        std::process::exit(2);
                    });
            }
            "--fps" => {
                fps = args
                    .next()
                    .and_then(|value| value.to_str()?.parse().ok())
                    .unwrap_or_else(|| {
                        eprintln!("--fps requires a positive integer");
                        std::process::exit(2);
                    });
                if fps == 0 {
                    eprintln!("--fps must be greater than zero");
                    std::process::exit(2);
                }
            }
            value if value.starts_with('-') => {
                eprintln!("unknown option: {value}");
                std::process::exit(2);
            }
            value => {
                if path.replace(std::ffi::OsString::from(value)).is_some() {
                    eprintln!("only one output file is allowed");
                    std::process::exit(2);
                }
            }
        }
    }
    let Some(path) = path else {
        eprintln!("usage: corten-record [--window] [--all] [--stream N] [--fps N] <output-file>");
        std::process::exit(2);
    };

    let mut session = match if all_streams {
        corten_capture_session::CaptureSession::start_all(monitor_only).await
    } else {
        corten_capture_session::CaptureSession::start_with_options(
            monitor_only,
            stream_index > 0,
            stream_index,
        )
        .await
    } {
        Ok(session) => session,
        Err(error) => {
            eprintln!("capture setup failed: {error}");
            std::process::exit(1);
        }
    };
    let stream_indices = if all_streams {
        (0..session.stream_count()).collect::<Vec<_>>()
    } else {
        vec![stream_index]
    };
    for index in stream_indices {
        let output = if all_streams {
            stream_output_path(&path, index)
        } else {
            path.clone()
        };
        let consumer = match corten_capture::FfmpegConsumer::new(output, fps) {
            Ok(consumer) => consumer,
            Err(error) => {
                eprintln!("encoder setup failed: {error}");
                let _ = session.stop().await;
                std::process::exit(1);
            }
        };
        if let Err(error) = session.attach_consumer(index, consumer) {
            eprintln!("consumer setup failed: {error}");
            let _ = session.stop().await;
            std::process::exit(1);
        }
    }

    eprintln!("recording; press Ctrl+C to stop");
    let _ = tokio::signal::ctrl_c().await;
    if let Err(error) = session.stop().await {
        eprintln!("capture shutdown failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(feature = "ffmpeg")]
fn stream_output_path(path: &std::ffi::OsStr, index: usize) -> std::path::PathBuf {
    let path = std::path::Path::new(path);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("recording");
    let suffix = path.extension().and_then(|value| value.to_str());
    let name = match suffix {
        Some(extension) => format!("{stem}.stream{index}.{extension}"),
        None => format!("{stem}.stream{index}"),
    };
    path.with_file_name(name)
}

#[cfg(not(feature = "ffmpeg"))]
fn main() {
    eprintln!("corten-record requires: cargo run -p corten-capture-session --features ffmpeg -- <output-file>");
    std::process::exit(2);
}
