//! Buffered reader for the server-to-client byte stream. All integers in the
//! RFB protocol are big-endian.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt};

const BUF_SIZE: usize = 64 * 1024;

pub struct RfbReader {
    inner: Box<dyn AsyncRead + Send + Unpin>,
    buf: Box<[u8]>,
    start: usize,
    end: usize,
    bytes_read: u64,
}

impl RfbReader {
    pub fn new(inner: Box<dyn AsyncRead + Send + Unpin>) -> Self {
        RfbReader { inner, buf: vec![0u8; BUF_SIZE].into_boxed_slice(), start: 0, end: 0, bytes_read: 0 }
    }

    /// Reader over a fixed block of bytes, handy for tests.
    pub fn from_bytes(data: Vec<u8>) -> Self {
        Self::new(Box::new(io::Cursor::new(data)))
    }

    /// Total bytes received from the network so far.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// Bytes already received but not yet consumed.
    pub fn buffered(&self) -> usize {
        self.end - self.start
    }

    /// Give back the underlying stream and any bytes read ahead.
    pub fn into_inner(self) -> (Box<dyn AsyncRead + Send + Unpin>, Vec<u8>) {
        let rest = self.buf[self.start..self.end].to_vec();
        (self.inner, rest)
    }

    async fn fill(&mut self) -> io::Result<()> {
        if self.start == self.end {
            self.start = 0;
            self.end = 0;
        }
        if self.end == self.buf.len() {
            self.buf.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
        }
        let n = self.inner.read(&mut self.buf[self.end..]).await?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the connection was closed"));
        }
        self.end += n;
        self.bytes_read += n as u64;
        Ok(())
    }

    pub async fn read_exact(&mut self, out: &mut [u8]) -> io::Result<()> {
        let mut done = 0;
        while done < out.len() {
            if self.start == self.end {
                // Large reads go straight into the caller's buffer.
                if out.len() - done >= self.buf.len() {
                    let n = self.inner.read(&mut out[done..]).await?;
                    if n == 0 {
                        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the connection was closed"));
                    }
                    self.bytes_read += n as u64;
                    done += n;
                    continue;
                }
                self.fill().await?;
            }
            let n = (self.end - self.start).min(out.len() - done);
            out[done..done + n].copy_from_slice(&self.buf[self.start..self.start + n]);
            self.start += n;
            done += n;
        }
        Ok(())
    }

    pub async fn read_vec(&mut self, n: usize) -> io::Result<Vec<u8>> {
        let mut v = vec![0u8; n];
        self.read_exact(&mut v).await?;
        Ok(v)
    }

    pub async fn skip(&mut self, mut n: usize) -> io::Result<()> {
        while n > 0 {
            if self.start == self.end {
                self.fill().await?;
            }
            let k = (self.end - self.start).min(n);
            self.start += k;
            n -= k;
        }
        Ok(())
    }

    pub async fn read_u8(&mut self) -> io::Result<u8> {
        if self.start == self.end {
            self.fill().await?;
        }
        let b = self.buf[self.start];
        self.start += 1;
        Ok(b)
    }

    pub async fn read_u16(&mut self) -> io::Result<u16> {
        let mut b = [0u8; 2];
        self.read_exact(&mut b).await?;
        Ok(u16::from_be_bytes(b))
    }

    pub async fn read_u32(&mut self) -> io::Result<u32> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b).await?;
        Ok(u32::from_be_bytes(b))
    }

    pub async fn read_i32(&mut self) -> io::Result<i32> {
        Ok(self.read_u32().await? as i32)
    }

    /// Read a Tight-style "compact length" (1 to 3 bytes, 7 bits each).
    pub async fn read_compact_len(&mut self) -> io::Result<usize> {
        let b0 = self.read_u8().await? as usize;
        let mut len = b0 & 0x7f;
        if b0 & 0x80 != 0 {
            let b1 = self.read_u8().await? as usize;
            len |= (b1 & 0x7f) << 7;
            if b1 & 0x80 != 0 {
                let b2 = self.read_u8().await? as usize;
                len |= b2 << 14;
            }
        }
        Ok(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_integers_and_compact_lengths() {
        let mut r = RfbReader::from_bytes(vec![0x01, 0x02, 0, 0, 0, 5, 0x90, 0x4e, 0xff]);
        assert_eq!(r.read_u16().await.unwrap(), 0x0102);
        assert_eq!(r.read_u32().await.unwrap(), 5);
        assert_eq!(r.read_compact_len().await.unwrap(), 10000);
        assert_eq!(r.read_u8().await.unwrap(), 0xff);
        assert!(r.read_u8().await.is_err());
    }
}
