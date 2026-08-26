use std::{io, sync::Arc};

use fixtrace::application::FixTraceProtocolApplication;
use fixtrace_protocol::{AppErrorView, ErrorCode, ResponseEnvelope, ServerFrame};
use futures_util::StreamExt;
use thiserror::Error;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt, BufWriter},
    sync::broadcast,
};
use tokio_util::{
    codec::{FramedRead, LinesCodec, LinesCodecError},
    sync::CancellationToken,
};
use uuid::Uuid;

use crate::{ConnectionAction, ConnectionState, MAX_FRAME_BYTES};

#[derive(Debug, Error)]
pub enum StdioError {
    #[error("stdio transport failed: {0}")]
    Io(#[from] io::Error),
    #[error("could not serialize server frame: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Serves newline-delimited JSON on stdin/stdout. Diagnostics must be written to stderr.
pub async fn serve_stdio(
    application: Arc<dyn FixTraceProtocolApplication>,
    cancellation: CancellationToken,
) -> Result<(), StdioError> {
    let mut events = application.subscribe_protocol_events();
    let mut connection = ConnectionState::new(application);
    let mut input = FramedRead::new(
        tokio::io::stdin(),
        LinesCodec::new_with_max_length(MAX_FRAME_BYTES),
    );
    let mut output = BufWriter::new(tokio::io::stdout());

    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            next = input.next() => match next {
                Some(Ok(line)) => {
                    let reply = connection.handle_text(&line).await;
                    write_frame(&mut output, &reply.frame).await?;
                    if matches!(reply.action, ConnectionAction::Close) {
                        break;
                    }
                    match connection.apply_action(reply.action) {
                        Ok(frames) => write_frames(&mut output, frames).await?,
                        Err(error) => write_frame(&mut output, &error_frame(error)).await?,
                    }
                }
                Some(Err(LinesCodecError::MaxLineLengthExceeded)) => {
                    write_frame(
                        &mut output,
                        &error_frame(AppErrorView::new(
                            ErrorCode::FrameTooLarge,
                            format!("frame exceeds {MAX_FRAME_BYTES} bytes"),
                        )),
                    ).await?;
                }
                Some(Err(LinesCodecError::Io(error))) => return Err(error.into()),
                None => break,
            },
            event = events.recv() => match event {
                Ok(event) => {
                    let frames = connection.on_live_event(event).unwrap_or_else(|error| {
                        vec![error_frame(error)]
                    });
                    write_frames(&mut output, frames).await?;
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let frames = connection.recover_lagged().unwrap_or_else(|error| {
                        vec![error_frame(error)]
                    });
                    write_frames(&mut output, frames).await?;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
    output.flush().await?;
    Ok(())
}

async fn write_frames<W>(writer: &mut W, frames: Vec<ServerFrame>) -> Result<(), StdioError>
where
    W: AsyncWrite + Unpin,
{
    for frame in frames {
        write_frame(writer, &frame).await?;
    }
    Ok(())
}

async fn write_frame<W>(writer: &mut W, frame: &ServerFrame) -> Result<(), StdioError>
where
    W: AsyncWrite + Unpin,
{
    let bytes = serde_json::to_vec(frame)?;
    writer.write_all(&bytes).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

fn error_frame(error: AppErrorView) -> ServerFrame {
    ServerFrame::Response(ResponseEnvelope::error(Uuid::nil(), error))
}
