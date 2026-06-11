use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::net::protocol::{NetworkMessage, PROTOCOL_MAGIC};

/// A TCP stream framed with length-delimited encoding for message boundaries.
pub type MessageFramed = Framed<TcpStream, LengthDelimitedCodec>;

/// Wrap a `TcpStream` in a length-delimited codec frame.
pub fn create_framed(stream: TcpStream) -> MessageFramed {
    Framed::new(stream, LengthDelimitedCodec::new())
}

/// Serialize and send a [`NetworkMessage`] over a framed TCP connection.
///
/// Prepends [`PROTOCOL_MAGIC`] before the bincode payload.
pub async fn send_message(
    framed: &mut MessageFramed,
    msg: &NetworkMessage,
) -> Result<(), Box<dyn std::error::Error>> {
    let bincode_bytes = bincode::serde::encode_to_vec(msg, bincode::config::standard())?;
    let mut payload = Vec::with_capacity(PROTOCOL_MAGIC.len() + bincode_bytes.len());
    payload.extend_from_slice(&PROTOCOL_MAGIC);
    payload.extend_from_slice(&bincode_bytes);
    framed.send(payload.into()).await?;
    Ok(())
}

/// Receive and deserialize a [`NetworkMessage`] from a framed TCP connection.
///
/// Verifies the first 8 bytes match [`PROTOCOL_MAGIC`] before decoding.
/// Returns `Ok(None)` when the remote end has closed the connection.
pub async fn recv_message(
    framed: &mut MessageFramed,
) -> Result<Option<NetworkMessage>, Box<dyn std::error::Error>> {
    match framed.next().await {
        Some(Ok(bytes)) => {
            if bytes.len() < PROTOCOL_MAGIC.len()
                || bytes[..PROTOCOL_MAGIC.len()] != PROTOCOL_MAGIC
            {
                return Err("Invalid protocol magic bytes in TCP frame".into());
            }
            let (msg, _) = bincode::serde::decode_from_slice(
                &bytes[PROTOCOL_MAGIC.len()..],
                bincode::config::standard(),
            )?;
            Ok(Some(msg))
        }
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}
