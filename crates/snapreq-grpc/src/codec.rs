//! A tonic codec for messages known only at run time.

use bytes::Buf;
use prost::Message as _;
use prost_reflect::{DynamicMessage, MessageDescriptor};
use tonic::Status;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};

/// Encodes [`DynamicMessage`]s and decodes them as the method's output type.
#[derive(Debug, Clone)]
pub struct DynamicCodec {
    output: MessageDescriptor,
}

impl DynamicCodec {
    pub fn new(output: MessageDescriptor) -> Self {
        Self { output }
    }
}

#[derive(Debug)]
pub struct DynamicEncoder;

#[derive(Debug)]
pub struct DynamicDecoder(MessageDescriptor);

impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynamicEncoder;
    type Decoder = DynamicDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        DynamicEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        DynamicDecoder(self.output.clone())
    }
}

impl Encoder for DynamicEncoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn encode(&mut self, item: Self::Item, dst: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        item.encode(dst)
            .map_err(|e| Status::internal(format!("cannot encode the message: {e}")))
    }
}

impl Decoder for DynamicDecoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        let mut msg = DynamicMessage::new(self.0.clone());
        msg.merge(src.copy_to_bytes(src.remaining()))
            .map_err(|e| Status::internal(format!("cannot decode the message: {e}")))?;
        Ok(Some(msg))
    }
}
