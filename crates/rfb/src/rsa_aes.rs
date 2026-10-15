//! RSA-AES login (security types 5, 6, 129 and 130), the encrypted login
//! method the Raspberry Pi's wayvnc server offers by default.
//!
//! Both sides exchange RSA public keys and random numbers, derive AES keys
//! from them and then wrap every message in AES-EAX. See the "RSA-AES
//! Security Type" section of the RFB specification.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use aes::{Aes128, Aes256};
use eax::Eax;
use eax::aead::{AeadInPlace, KeyInit, generic_array::GenericArray};
use rand::RngCore;
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::error::SessionError;
use crate::handshake::Prompter;
use crate::session::{BoxedStream, IdentityKind};

pub const RA2: u8 = 5;
pub const RA2NE: u8 = 6;
pub const RA2_256: u8 = 129;
pub const RA2NE_256: u8 = 130;

const CLIENT_KEY_BITS: usize = 2048;
const MAX_FRAME: usize = 8192;

/// Run the RSA-AES login. Returns the stream to use from now on: wrapped in
/// AES-EAX for types 5 and 129, plain for the "unencrypted" types 6 and 130.
pub async fn authenticate(
    mut stream: BoxedStream,
    sec_type: u8,
    ui: &mut Prompter<'_>,
) -> Result<BoxedStream, SessionError> {
    let wide = matches!(sec_type, RA2_256 | RA2NE_256);
    let keep_encryption = matches!(sec_type, RA2 | RA2_256);

    // Start making our own key while the server's key arrives.
    let client_key = tokio::task::spawn_blocking(|| RsaPrivateKey::new(&mut rand::thread_rng(), CLIENT_KEY_BITS));

    let server_bits = stream.read_u32().await? as usize;
    if !(1024..=8192).contains(&server_bits) {
        return Err(SessionError::protocol(format!("the server's encryption key has an unusual size ({server_bits} bits)")));
    }
    let n_len = server_bits.div_ceil(8);
    let mut server_pub_msg = vec![0u8; 4 + n_len * 2];
    server_pub_msg[..4].copy_from_slice(&(server_bits as u32).to_be_bytes());
    stream.read_exact(&mut server_pub_msg[4..]).await?;
    let server_key = RsaPublicKey::new_with_max_size(
        BigUint::from_bytes_be(&server_pub_msg[4..4 + n_len]),
        BigUint::from_bytes_be(&server_pub_msg[4 + n_len..]),
        8192,
    )
    .map_err(|e| SessionError::protocol(format!("the server's encryption key is not valid ({e})")))?;

    let fingerprint = crate::handshake::fingerprint(&Sha256::digest(&server_pub_msg[4..]));
    ui.confirm_identity(IdentityKind::RsaKey, fingerprint).await?;

    let client_key = client_key
        .await
        .map_err(|e| SessionError::protocol(format!("could not create an encryption key ({e})")))?
        .map_err(|e| SessionError::protocol(format!("could not create an encryption key ({e})")))?;
    let client_pub_msg = public_key_message(&client_key.to_public_key());
    stream.write_all(&client_pub_msg).await?;
    stream.flush().await?;

    // Server random, encrypted with our public key.
    let len = stream.read_u16().await? as usize;
    if len != CLIENT_KEY_BITS / 8 {
        return Err(SessionError::protocol("the server's encrypted challenge has the wrong size"));
    }
    let mut encrypted = vec![0u8; len];
    stream.read_exact(&mut encrypted).await?;
    let server_random = client_key
        .decrypt(Pkcs1v15Encrypt, &encrypted)
        .map_err(|_| SessionError::protocol("could not decrypt the server's challenge"))?;
    let random_len = if wide { 32 } else { 16 };
    if server_random.len() != random_len {
        return Err(SessionError::protocol("the server's challenge has the wrong size"));
    }

    // Our random, encrypted with the server's public key.
    let mut client_random = vec![0u8; random_len];
    rand::thread_rng().fill_bytes(&mut client_random);
    let encrypted = server_key
        .encrypt(&mut rand::thread_rng(), Pkcs1v15Encrypt, &client_random)
        .map_err(|e| SessionError::protocol(format!("could not encrypt our challenge ({e})")))?;
    let mut msg = (encrypted.len() as u16).to_be_bytes().to_vec();
    msg.extend_from_slice(&encrypted);
    stream.write_all(&msg).await?;
    stream.flush().await?;

    let client_session_key = hash(wide, &[&server_random, &client_random]);
    let server_session_key = hash(wide, &[&client_random, &server_random]);
    let key_len = if wide { 32 } else { 16 };
    let mut secure = EaxStream::new(stream, &client_session_key[..key_len], &server_session_key[..key_len]);

    // Each side proves it saw the same two public keys.
    let hash_len = if wide { 32 } else { 20 };
    let mut server_hash = vec![0u8; hash_len];
    secure.read_exact(&mut server_hash).await?;
    if server_hash != hash(wide, &[&server_pub_msg, &client_pub_msg]) {
        return Err(SessionError::protocol("the encrypted connection could not be verified"));
    }
    secure.write_all(&hash(wide, &[&client_pub_msg, &server_pub_msg])).await?;
    secure.flush().await?;

    let subtype = secure.read_u8().await?;
    let need_username = match subtype {
        1 => true,
        2 => false,
        other => return Err(SessionError::protocol(format!("unknown login request ({other})"))),
    };
    let (username, password) = ui.credentials(need_username).await?;
    let username = if need_username { username.unwrap_or_default() } else { String::new() };
    let mut creds = Vec::with_capacity(2 + username.len() + password.len());
    push_short_string(&mut creds, &username)?;
    push_short_string(&mut creds, &password)?;
    secure.write_all(&creds).await?;
    secure.flush().await?;

    if keep_encryption { Ok(Box::new(secure)) } else { Ok(secure.into_inner()) }
}

fn push_short_string(out: &mut Vec<u8>, s: &str) -> Result<(), SessionError> {
    let b = s.as_bytes();
    if b.len() > 255 {
        return Err(SessionError::auth("the user name or password is too long (at most 255 bytes)"));
    }
    out.push(b.len() as u8);
    out.extend_from_slice(b);
    Ok(())
}

fn public_key_message(key: &RsaPublicKey) -> Vec<u8> {
    let bits = key.size() * 8;
    let n_len = key.size();
    let mut m = Vec::with_capacity(4 + n_len * 2);
    m.extend_from_slice(&(bits as u32).to_be_bytes());
    for part in [key.n(), key.e()] {
        let bytes = part.to_bytes_be();
        m.extend(std::iter::repeat_n(0u8, n_len - bytes.len()));
        m.extend_from_slice(&bytes);
    }
    m
}

fn hash(wide: bool, parts: &[&[u8]]) -> Vec<u8> {
    if wide {
        let mut h = Sha256::new();
        parts.iter().for_each(|p| h.update(p));
        h.finalize().to_vec()
    } else {
        let mut h = Sha1::new();
        parts.iter().for_each(|p| h.update(p));
        h.finalize().to_vec()
    }
}

enum Cipher {
    Aes128(Box<Eax<Aes128>>),
    Aes256(Box<Eax<Aes256>>),
}

impl Cipher {
    fn new(key: &[u8]) -> Self {
        if key.len() == 32 {
            Cipher::Aes256(Box::new(Eax::new(GenericArray::from_slice(key))))
        } else {
            Cipher::Aes128(Box::new(Eax::new(GenericArray::from_slice(key))))
        }
    }

    fn encrypt(&self, nonce: &[u8; 16], aad: &[u8], buf: &mut Vec<u8>) -> io::Result<()> {
        let nonce = GenericArray::from_slice(nonce);
        let tag = match self {
            Cipher::Aes128(c) => c.encrypt_in_place_detached(nonce, aad, buf),
            Cipher::Aes256(c) => c.encrypt_in_place_detached(nonce, aad, buf),
        }
        .map_err(|_| io::Error::other("encryption failed"))?;
        buf.extend_from_slice(&tag);
        Ok(())
    }

    fn decrypt(&self, nonce: &[u8; 16], aad: &[u8], buf: &mut [u8], tag: &[u8]) -> io::Result<()> {
        let nonce = GenericArray::from_slice(nonce);
        let tag = GenericArray::from_slice(tag);
        match self {
            Cipher::Aes128(c) => c.decrypt_in_place_detached(nonce, aad, buf, tag),
            Cipher::Aes256(c) => c.decrypt_in_place_detached(nonce, aad, buf, tag),
        }
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a message failed its integrity check"))
    }
}

fn nonce(counter: u128) -> [u8; 16] {
    counter.to_le_bytes()
}

/// A byte stream where every chunk is encrypted with AES-EAX and framed as
/// (u16 length, ciphertext, 16 byte tag). Reads never consume more bytes
/// from the inner stream than the current frame needs.
pub struct EaxStream {
    inner: BoxedStream,
    send: Cipher,
    recv: Cipher,
    send_counter: u128,
    recv_counter: u128,
    // Reading.
    raw: Vec<u8>,
    plain: Vec<u8>,
    plain_pos: usize,
    // Writing.
    out_plain: Vec<u8>,
    out_cipher: Vec<u8>,
    out_pos: usize,
}

impl EaxStream {
    fn new(inner: BoxedStream, send_key: &[u8], recv_key: &[u8]) -> Self {
        EaxStream {
            inner,
            send: Cipher::new(send_key),
            recv: Cipher::new(recv_key),
            send_counter: 0,
            recv_counter: 0,
            raw: Vec::new(),
            plain: Vec::new(),
            plain_pos: 0,
            out_plain: Vec::new(),
            out_cipher: Vec::new(),
            out_pos: 0,
        }
    }

    fn into_inner(self) -> BoxedStream {
        self.inner
    }

    fn seal_pending(&mut self) -> io::Result<()> {
        if self.out_pos == self.out_cipher.len() {
            self.out_cipher.clear();
            self.out_pos = 0;
        }
        let plain = std::mem::take(&mut self.out_plain);
        for chunk in plain.chunks(MAX_FRAME) {
            let header = (chunk.len() as u16).to_be_bytes();
            let mut buf = chunk.to_vec();
            self.send.encrypt(&nonce(self.send_counter), &header, &mut buf)?;
            self.send_counter += 1;
            self.out_cipher.extend_from_slice(&header);
            self.out_cipher.extend_from_slice(&buf);
        }
        Ok(())
    }

    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.out_pos < self.out_cipher.len() {
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.out_cipher[self.out_pos..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.out_pos += n;
        }
        self.out_cipher.clear();
        self.out_pos = 0;
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for EaxStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        loop {
            if this.plain_pos < this.plain.len() {
                let n = (this.plain.len() - this.plain_pos).min(buf.remaining());
                buf.put_slice(&this.plain[this.plain_pos..this.plain_pos + n]);
                this.plain_pos += n;
                return Poll::Ready(Ok(()));
            }
            let need = if this.raw.len() < 2 {
                2
            } else {
                2 + u16::from_be_bytes([this.raw[0], this.raw[1]]) as usize + 16
            };
            if this.raw.len() < need {
                let have = this.raw.len();
                this.raw.resize(need, 0);
                let mut rb = ReadBuf::new(&mut this.raw[have..need]);
                let res = Pin::new(&mut this.inner).poll_read(cx, &mut rb);
                let got = rb.filled().len();
                this.raw.truncate(have + got);
                ready!(res)?;
                if got == 0 {
                    if have == 0 {
                        return Poll::Ready(Ok(()));
                    }
                    return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()));
                }
                continue;
            }
            let len = need - 18;
            let header = [this.raw[0], this.raw[1]];
            let (body, tag) = this.raw[2..].split_at_mut(len);
            this.recv.decrypt(&nonce(this.recv_counter), &header, body, tag)?;
            this.recv_counter += 1;
            this.plain.clear();
            this.plain.extend_from_slice(body);
            this.plain_pos = 0;
            this.raw.clear();
        }
    }
}

impl AsyncWrite for EaxStream {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        if this.out_plain.len() >= 64 * 1024 {
            this.seal_pending()?;
            ready!(this.poll_drain(cx))?;
        }
        this.out_plain.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if !this.out_plain.is_empty() {
            this.seal_pending()?;
        }
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.as_mut().poll_flush(cx))?;
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn eax_stream_round_trip() {
        let (a, b) = tokio::io::duplex(1 << 20);
        let k1 = [1u8; 32];
        let k2 = [2u8; 32];
        let mut left = EaxStream::new(Box::new(a), &k1, &k2);
        let mut right = EaxStream::new(Box::new(b), &k2, &k1);
        let big: Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
        left.write_all(b"hello").await.unwrap();
        left.write_all(&big).await.unwrap();
        left.flush().await.unwrap();
        let mut got = vec![0u8; 5 + big.len()];
        right.read_exact(&mut got).await.unwrap();
        assert_eq!(&got[..5], b"hello");
        assert_eq!(&got[5..], &big[..]);
        right.write_all(b"ok").await.unwrap();
        right.flush().await.unwrap();
        let mut two = [0u8; 2];
        left.read_exact(&mut two).await.unwrap();
        assert_eq!(&two, b"ok");
    }

    #[test]
    fn public_key_message_pads_exponent() {
        let key = RsaPrivateKey::new(&mut rand::thread_rng(), 1024).unwrap().to_public_key();
        let m = public_key_message(&key);
        assert_eq!(m.len(), 4 + 128 * 2);
        assert_eq!(&m[..4], &1024u32.to_be_bytes());
        assert_eq!(&m[4 + 128 + 125..], &[1, 0, 1]);
    }
}
