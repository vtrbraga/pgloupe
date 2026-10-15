//! A bare-bones protocol client that can hang up without saying goodbye.
//!
//! The `postgres` crate always sends `Terminate`; a client that vanishes (a crashed app, a dropped
//! network) does not, and the server logs it differently.

use std::{
    io::{Read as _, Write as _},
    net::TcpStream,
};

use anyhow::{Context as _, bail};

/// A connection in trust-authentication mode.
#[derive(Debug)]
pub struct Raw {
    stream: TcpStream,
}

impl Raw {
    /// Connects and completes the startup handshake.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors or when the server asks for a password.
    pub fn connect(port: u16, db: &str, user: &str, app: &str) -> anyhow::Result<Raw> {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).context("connecting")?;
        let mut body = 196_608_i32.to_be_bytes().to_vec();
        for (key, value) in [("user", user), ("database", db), ("application_name", app)] {
            body.extend_from_slice(key.as_bytes());
            body.push(0);
            body.extend_from_slice(value.as_bytes());
            body.push(0);
        }
        body.push(0);
        let length = i32::try_from(body.len() + 4).context("startup message too long")?;
        stream.write_all(&length.to_be_bytes())?;
        stream.write_all(&body)?;
        let mut raw = Raw { stream };
        raw.until_ready()?;
        Ok(raw)
    }

    /// Runs `sql` with the simple protocol and waits for the server to be ready again.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors or an error response.
    pub fn query(&mut self, sql: &str) -> anyhow::Result<()> {
        let mut message = vec![b'Q'];
        let length = i32::try_from(sql.len() + 5).context("query too long")?;
        message.extend_from_slice(&length.to_be_bytes());
        message.extend_from_slice(sql.as_bytes());
        message.push(0);
        self.stream.write_all(&message)?;
        self.until_ready()
    }

    /// Closes the socket without a `Terminate` message.
    pub fn hang_up(self) {
        drop(self.stream.shutdown(std::net::Shutdown::Both));
    }

    fn until_ready(&mut self) -> anyhow::Result<()> {
        loop {
            let mut head = [0_u8; 5];
            self.stream.read_exact(&mut head)?;
            let length = usize::try_from(i32::from_be_bytes([head[1], head[2], head[3], head[4]]))
                .context("bad message length")?;
            let mut body = vec![0_u8; length.saturating_sub(4)];
            self.stream.read_exact(&mut body)?;
            match head[0] {
                b'Z' => return Ok(()),
                b'E' => bail!("server error: {}", String::from_utf8_lossy(&body)),
                b'R' if body.first() != Some(&0) => {
                    bail!("the server wants authentication; the generator expects trust");
                }
                _ => {}
            }
        }
    }
}
