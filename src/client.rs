use std::{io, mem, result, str, sync::LazyLock};

use compio::{io::{util::Splittable, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt}, BufResult};
use rand::{Rng, SeedableRng, rngs::SmallRng};

use crate::{CloseCode, Frame, Opcode};

pub static PROTOCOL_ERROR: LazyLock<Vec<u8>> = LazyLock::new(|| {
    u16::from(CloseCode::ProtocolError)
        .to_be_bytes()
        .into_iter()
        .collect()
});

pub struct Config {
    pub read_buffer_capacity: usize,
    pub write_buffer_capacity: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            read_buffer_capacity: 128 * 1024,
            write_buffer_capacity: 128 * 1024,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("IO: {0}")]
    Io(#[from] io::Error),
    #[error("Protocol violation: {0}")]
    ProtocolViolation(&'static str),
    #[error("The connection has been closed: {code:?} {reason:?}.")]
    Closed {
        code: Option<CloseCode>,
        reason: Option<String>,
    },
}

pub type BufResult<T> = (result::Result<T, Error>, Vec<u8>);
pub type Result<T> = result::Result<T, Error>;

pub struct Client<S>
// where
//     S: AsyncWrite,
{
    stream: S,
    read_buffer: Vec<u8>,
    read_consumed: usize,
    write_buffer: Vec<u8>,
    write_rng: SmallRng,
    // read_half: ReadHalf<S>,
    // write_half: WriteHalf<S>,
}

impl<S> Client<S> {
    pub fn new(stream: S, config: &Config) -> Self {
        Self {
            stream,
            read_buffer: Vec::with_capacity(config.read_buffer_capacity),
            read_consumed: 0,
            write_buffer: Vec::with_capacity(config.write_buffer_capacity),
            write_rng: SmallRng::from_os_rng(),
            // read_half: ReadHalf {
            //     inner: read_half,
            //     buffer: Vec::with_capacity(config.read_buffer_capacity),
            //     consumed: 0,
            // },
            // write_half: WriteHalf {
            //     inner: write_half,
            //     rng: SmallRng::from_os_rng(),
            //     buffer: Vec::with_capacity(config.write_buffer_capacity),
            // },
        }
    }
}

impl<S> Client<S>
where
    S: AsyncRead,
{
    const CHUNK_SIZE: usize = 4096;

    #[inline]
    async fn read_frame_inner(&mut self) -> Result<Frame> {
        const HEADER_LEN: usize = 2;

        if self.read_consumed > 0
            && self.read_buffer.len() > self.read_buffer.capacity() - Self::CHUNK_SIZE
        {
            self.read_buffer.drain(..self.read_consumed);
            self.read_consumed = 0;
        }

        self.ensure_read(HEADER_LEN).await?;

        let b1 = self.read_buffer[self.read_consumed];
        let b2 = self.read_buffer[self.read_consumed + 1];
        self.read_consumed += HEADER_LEN;

        let fin = b1 & 0x80 != 0;
        let rsv = b1 & 0x70;
        let opcode = unsafe { mem::transmute::<u8, Opcode>(b1 & 0x0F) };
        let masked = b2 & 0x80 != 0;
        let mut length = (b2 & 0x7F) as usize;

        if rsv != 0 {
            return Err(Error::ProtocolViolation("Reserve bit must be 0."));
        }
        if masked {
            return Err(Error::ProtocolViolation(
                "Server to client communication should be unmasked.",
            ));
        }

        match opcode {
            Opcode::Reserved3
            | Opcode::Reserved4
            | Opcode::Reserved5
            | Opcode::Reserved6
            | Opcode::Reserved7
            | Opcode::ReservedB
            | Opcode::ReservedC
            | Opcode::ReservedD
            | Opcode::ReservedE
            | Opcode::ReservedF => {
                return Err(Error::ProtocolViolation("Use of reserved opcode."));
            }
            Opcode::Close => {
                if length == 1 {
                    return Err(Error::ProtocolViolation(
                        "Close frame with a missing close reason byte.",
                    ));
                }
                if length > 125 {
                    return Err(Error::ProtocolViolation(
                        "Control frame larger than 125 bytes.",
                    ));
                }
                if !fin {
                    return Err(Error::ProtocolViolation(
                        "Control frame cannot be fragmented.",
                    ));
                }
            }
            Opcode::Ping | Opcode::Pong => {
                if length > 125 {
                    return Err(Error::ProtocolViolation(
                        "Control frame larger than 125 bytes.",
                    ));
                }
                if !fin {
                    return Err(Error::ProtocolViolation(
                        "Control frame cannot be fragmented.",
                    ));
                }
            }
            Opcode::Text | Opcode::Binary | Opcode::Continuation => {
                length = match length {
                    126 => {
                        const LENGTH_LEN: usize = 2;

                        self.ensure_read(LENGTH_LEN).await?;

                        let mut bytes = [0u8; LENGTH_LEN];
                        bytes.copy_from_slice(
                            &self.read_buffer[self.read_consumed..self.read_consumed + LENGTH_LEN],
                        );
                        self.read_consumed += LENGTH_LEN;
                        u16::from_be_bytes(bytes) as usize
                    }
                    127 => {
                        const LENGTH_LEN: usize = 8;

                        self.ensure_read(LENGTH_LEN).await?;

                        let mut bytes = [0u8; LENGTH_LEN];
                        bytes.copy_from_slice(
                            &self.read_buffer[self.read_consumed..self.read_consumed + LENGTH_LEN],
                        );
                        self.read_consumed += LENGTH_LEN;
                        u64::from_be_bytes(bytes) as usize
                    }
                    length => length,
                };
            }
        }

        self.ensure_read(length).await?;

        let data = &self.read_buffer[self.read_consumed..self.read_consumed + length];
        self.read_consumed += length;

        Ok(Frame { fin, opcode, data })
    }

    #[inline]
    async fn ensure_read(&mut self, len: usize) -> Result<()> {
        while self.read_buffer.len() < self.read_consumed + len {
            let buffer = mem::take(&mut self.read_buffer);
            self.stream.read_exact()
            let (res, buffer) = self.stream.read_extend(buffer, Self::CHUNK_SIZE).await;
            self.read_buffer = buffer;
            let _ = res?;
        }
        Ok(())
    }
}

impl<S> Client<S>
where
    S: AsyncWrite,
{
    pub async fn send_ping(&mut self, data: &[u8]) -> io::Result<()> {
        self.send(Frame {
            fin: true,
            opcode: Opcode::Ping,
            data,
        })
        .await
    }

    pub async fn send_pong(&mut self, data: &[u8]) -> io::Result<()> {
        self.send(Frame {
            fin: true,
            opcode: Opcode::Pong,
            data,
        })
        .await
    }

    pub async fn send_binary(&mut self, data: &[u8]) -> io::Result<()> {
        self.send(Frame {
            fin: true,
            opcode: Opcode::Binary,
            data,
        })
        .await
    }

    pub async fn send_text(&mut self, data: &[u8]) -> io::Result<()> {
        self.send(Frame {
            fin: true,
            opcode: Opcode::Text,
            data,
        })
        .await
    }

    pub async fn send_close(&mut self, data: &[u8]) -> io::Result<()> {
        self.send(Frame {
            fin: true,
            opcode: Opcode::Close,
            data,
        })
        .await
    }

    #[inline]
    async fn send(&mut self, frame: Frame<'_>) -> io::Result<()> {
        self.write_frame(frame).await
    }

    pub async fn write_frame(&mut self, frame: Frame<'_>) -> io::Result<()> {
        let mut dst = mem::take(&mut self.write_buffer);
        frame.encode(&mut dst, self.write_rng.random::<u32>().to_ne_bytes());
        let BufResult(res, buffer) = self.stream.write_all(dst).await;
        self.write_buffer = buffer;
        res.map(|_| ())
    }

    pub async fn write_control_frame(&mut self, frame: Frame<'_>) -> io::Result<()> {
        let mut dst = mem::take(&mut self.write_buffer);
        frame.encode_control(&mut dst, self.write_rng.random::<u32>().to_ne_bytes());
        let BufResult(res, buffer) = self.stream.write_all(dst).await;
        self.write_buffer = buffer;
        res.map(|_| ())
    }
}
