use anyhow::Result;
use tokio::io::{self, AsyncWrite, AsyncWriteExt};

pub struct FrameSink {
    writer: Box<dyn AsyncWrite + Unpin + Send>,
}

impl FrameSink {
    pub fn stdout() -> Self {
        Self {
            writer: Box::new(io::stdout()),
        }
    }

    pub async fn write_nalu(&mut self, nalu: &[u8]) -> Result<()> {
        self.writer.write_all(&[0x00, 0x00, 0x00, 0x01]).await?;
        self.writer.write_all(nalu).await?;
        Ok(())
    }
}
