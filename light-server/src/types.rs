//! Strong byte-array domain types used at API boundaries.
//!
//! Many Bitcoin/light-sync values are 32 bytes. Keeping them as distinct
//! newtypes avoids accidentally passing a txid where a block hash, output-id
//! hash, or Silent Payment tweak public key is expected.

macro_rules! byte32_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name([u8; 32]);

        impl $name {
            pub const fn new(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }
            pub const fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }
            pub const fn into_inner(self) -> [u8; 32] {
                self.0
            }
        }

        impl From<[u8; 32]> for $name {
            fn from(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }
        }

        impl From<$name> for [u8; 32] {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl AsRef<[u8]> for $name {
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }
    };
}

byte32_type!(BlockHashBytes);
byte32_type!(TxidBytes);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TxTweak([u8; 33]);

impl TxTweak {
    pub const LEN: usize = 33;
    pub const fn new(bytes: [u8; 33]) -> Self {
        Self(bytes)
    }
    pub const fn as_bytes(&self) -> &[u8; 33] {
        &self.0
    }
    pub const fn into_inner(self) -> [u8; 33] {
        self.0
    }
}

impl From<[u8; 33]> for TxTweak {
    fn from(bytes: [u8; 33]) -> Self {
        Self(bytes)
    }
}

impl From<TxTweak> for [u8; 33] {
    fn from(value: TxTweak) -> Self {
        value.0
    }
}

impl AsRef<[u8]> for TxTweak {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
byte32_type!(OutputIdHash);
