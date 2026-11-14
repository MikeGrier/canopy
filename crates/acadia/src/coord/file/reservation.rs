// Copyright (c) Michael Grier.

//! The creation reservation's bytes: which implementation coordinates the root, the creation's
//! nonce, and every creation input.

use crate::coord::{CoordError, CoordResult, CreationNonce, ImplementationId};

/// The tags and widths of the reservation's encoding. Changing any value is a breaking change to
/// the format: a reservation written under one cannot be read under another.
mod format {
    /// The bytes every reservation begins with.
    pub const MAGIC: [u8; 4] = *b"ACRV";
    /// The encoding's version, after the magic.
    pub const VERSION: u8 = 1;
    /// Bytes of the nonce.
    pub const NONCE_LEN: usize = 16;
    /// Bytes of the inputs' length, little-endian.
    pub const INPUTS_LEN_WIDTH: usize = 8;
}

/// What a reservation records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reservation {
    pub(crate) implementation: ImplementationId,
    pub(crate) nonce: CreationNonce,
    pub(crate) inputs: Vec<u8>,
}

impl Reservation {
    /// The magic, the version, the identity's length in one byte and its bytes, the nonce, the
    /// inputs' length and the inputs.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let id = self.implementation.as_str().as_bytes();
        let mut bytes = Vec::with_capacity(
            format::MAGIC.len()
                + 2
                + id.len()
                + format::NONCE_LEN
                + format::INPUTS_LEN_WIDTH
                + self.inputs.len(),
        );
        bytes.extend_from_slice(&format::MAGIC);
        bytes.push(format::VERSION);
        bytes.push(u8::try_from(id.len()).expect("an identity is at most 64 bytes"));
        bytes.extend_from_slice(id);
        bytes.extend_from_slice(&self.nonce.to_bytes());
        bytes.extend_from_slice(&(self.inputs.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&self.inputs);
        bytes
    }

    /// The reservation `bytes` encode, refusing anything [`Reservation::encode`] did not write.
    pub(crate) fn decode(bytes: &[u8]) -> CoordResult<Reservation> {
        let malformed = || CoordError::Malformed {
            what: "reservation",
        };
        let mut rest = bytes;
        let mut take = |n: usize| -> CoordResult<&[u8]> {
            if rest.len() < n {
                return Err(malformed());
            }
            let (head, tail) = rest.split_at(n);
            rest = tail;
            Ok(head)
        };
        if take(format::MAGIC.len())? != format::MAGIC || take(1)? != [format::VERSION] {
            return Err(malformed());
        }
        let id_len = usize::from(take(1)?[0]);
        let id = std::str::from_utf8(take(id_len)?).map_err(|_| malformed())?;
        let implementation = ImplementationId::new(id).ok_or_else(malformed)?;
        let nonce: [u8; format::NONCE_LEN] = take(format::NONCE_LEN)?
            .try_into()
            .map_err(|_| malformed())?;
        let len: [u8; format::INPUTS_LEN_WIDTH] = take(format::INPUTS_LEN_WIDTH)?
            .try_into()
            .map_err(|_| malformed())?;
        let len = usize::try_from(u64::from_le_bytes(len)).map_err(|_| malformed())?;
        let inputs = take(len)?.to_vec();
        if !rest.is_empty() {
            return Err(malformed());
        }
        Ok(Reservation {
            implementation,
            nonce: CreationNonce::from_bytes(nonce),
            inputs,
        })
    }
}
