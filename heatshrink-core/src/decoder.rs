//! Decoder state machine, ported from `heatshrink_decoder.c`.

use crate::config::{
    decoder_params_ok, STATIC_INPUT_BUFFER_SIZE, STATIC_LOOKAHEAD_BITS, STATIC_WINDOW_BITS,
};
use crate::error::Error;

const HSDS_TAG_BIT: u8 = 0;
const HSDS_YIELD_LITERAL: u8 = 1;
const HSDS_BACKREF_INDEX_MSB: u8 = 2;
const HSDS_BACKREF_INDEX_LSB: u8 = 3;
const HSDS_BACKREF_COUNT_MSB: u8 = 4;
const HSDS_BACKREF_COUNT_LSB: u8 = 5;
const HSDS_YIELD_BACKREF: u8 = 6;

const NO_BITS: u16 = u16::MAX;

/// Scalar decoder state. The byte buffer is input bytes followed by the window.
#[derive(Clone, Debug)]
pub struct DecoderState {
    pub input_size: u16,
    pub input_index: u16,
    pub output_count: u16,
    pub output_index: u16,
    pub head_index: u16,
    pub state: u8,
    pub current_byte: u8,
    pub bit_index: u8,
    pub window_sz2: u8,
    pub lookahead_sz2: u8,
    pub input_buffer_size: u16,
}

impl DecoderState {
    pub fn fresh(input_buffer_size: u16, window_sz2: u8, lookahead_sz2: u8) -> Self {
        Self {
            input_size: 0,
            input_index: 0,
            output_count: 0,
            output_index: 0,
            head_index: 0,
            state: HSDS_TAG_BIT,
            current_byte: 0,
            bit_index: 0,
            window_sz2,
            lookahead_sz2,
            input_buffer_size,
        }
    }

    pub fn reset(&mut self, buffers: &mut [u8]) {
        let buf_sz = (1usize << self.window_sz2) + self.input_buffer_size as usize;
        buffers[..buf_sz].fill(0);
        self.state = HSDS_TAG_BIT;
        self.input_size = 0;
        self.input_index = 0;
        self.bit_index = 0;
        self.current_byte = 0;
        self.output_count = 0;
        self.output_index = 0;
        self.head_index = 0;
    }

    /// `Ok(n)` is `HSDR_SINK_OK` with `n` bytes copied. `Err(Full)` is `HSDR_SINK_FULL`.
    pub fn sink(&mut self, buffers: &mut [u8], input: &[u8]) -> Result<usize, SinkFull> {
        let rem = self.input_buffer_size.wrapping_sub(self.input_size);
        if rem == 0 {
            return Err(SinkFull);
        }
        let n = std::cmp::min(rem as usize, input.len());
        let dst = self.input_size as usize;
        buffers[dst..dst + n].copy_from_slice(&input[..n]);
        self.input_size = self.input_size.wrapping_add(n as u16);
        Ok(n)
    }

    pub fn poll(
        &mut self,
        buffers: &mut [u8],
        out: &mut [u8],
    ) -> Result<crate::encoder::PollBytes, Error> {
        let mut produced = 0usize;
        loop {
            let in_state = self.state;
            self.state = match in_state {
                HSDS_TAG_BIT => self.tag_bit(buffers),
                HSDS_YIELD_LITERAL => self.yield_literal(buffers, out, &mut produced),
                HSDS_BACKREF_INDEX_MSB => self.backref_index_msb(buffers),
                HSDS_BACKREF_INDEX_LSB => self.backref_index_lsb(buffers),
                HSDS_BACKREF_COUNT_MSB => self.backref_count_msb(buffers),
                HSDS_BACKREF_COUNT_LSB => self.backref_count_lsb(buffers),
                HSDS_YIELD_BACKREF => self.yield_backref(buffers, out, &mut produced),
                _ => return Err(Error::Misuse),
            };
            if self.state == in_state {
                if produced == out.len() {
                    return Ok(crate::encoder::PollBytes {
                        more: true,
                        n: produced,
                    });
                }
                return Ok(crate::encoder::PollBytes {
                    more: false,
                    n: produced,
                });
            }
        }
    }

    /// `true` when finished (`HSDR_FINISH_DONE`).
    pub fn finish(&self) -> bool {
        match self.state {
            HSDS_TAG_BIT
            | HSDS_BACKREF_INDEX_LSB
            | HSDS_BACKREF_INDEX_MSB
            | HSDS_BACKREF_COUNT_LSB
            | HSDS_BACKREF_COUNT_MSB
            | HSDS_YIELD_LITERAL => self.input_size == 0,
            _ => false,
        }
    }

    fn tag_bit(&mut self, buffers: &[u8]) -> u8 {
        let bits = self.get_bits(buffers, 1);
        if bits == NO_BITS {
            HSDS_TAG_BIT
        } else if bits != 0 {
            HSDS_YIELD_LITERAL
        } else if self.window_sz2 > 8 {
            HSDS_BACKREF_INDEX_MSB
        } else {
            self.output_index = 0;
            HSDS_BACKREF_INDEX_LSB
        }
    }

    fn yield_literal(&mut self, buffers: &mut [u8], out: &mut [u8], produced: &mut usize) -> u8 {
        if *produced < out.len() {
            let byte = self.get_bits(buffers, 8);
            if byte == NO_BITS {
                return HSDS_YIELD_LITERAL;
            }
            let mask = (1u16 << self.window_sz2).wrapping_sub(1);
            let c = (byte & 0xff) as u8;
            let win = self.input_buffer_size as usize;
            let slot = (self.head_index & mask) as usize;
            buffers[win + slot] = c;
            self.head_index = self.head_index.wrapping_add(1);
            out[*produced] = c;
            *produced += 1;
            HSDS_TAG_BIT
        } else {
            HSDS_YIELD_LITERAL
        }
    }

    fn backref_index_msb(&mut self, buffers: &[u8]) -> u8 {
        let bit_ct = self.window_sz2;
        let bits = self.get_bits(buffers, bit_ct - 8);
        if bits == NO_BITS {
            return HSDS_BACKREF_INDEX_MSB;
        }
        self.output_index = bits << 8;
        HSDS_BACKREF_INDEX_LSB
    }

    fn backref_index_lsb(&mut self, buffers: &[u8]) -> u8 {
        let bit_ct = self.window_sz2;
        let take = if bit_ct < 8 { bit_ct } else { 8 };
        let bits = self.get_bits(buffers, take);
        if bits == NO_BITS {
            return HSDS_BACKREF_INDEX_LSB;
        }
        self.output_index |= bits;
        self.output_index = self.output_index.wrapping_add(1);
        self.output_count = 0;
        if self.lookahead_sz2 > 8 {
            HSDS_BACKREF_COUNT_MSB
        } else {
            HSDS_BACKREF_COUNT_LSB
        }
    }

    fn backref_count_msb(&mut self, buffers: &[u8]) -> u8 {
        let bits = self.get_bits(buffers, self.lookahead_sz2 - 8);
        if bits == NO_BITS {
            return HSDS_BACKREF_COUNT_MSB;
        }
        self.output_count = bits << 8;
        HSDS_BACKREF_COUNT_LSB
    }

    fn backref_count_lsb(&mut self, buffers: &[u8]) -> u8 {
        let br_bit_ct = self.lookahead_sz2;
        let take = if br_bit_ct < 8 { br_bit_ct } else { 8 };
        let bits = self.get_bits(buffers, take);
        if bits == NO_BITS {
            return HSDS_BACKREF_COUNT_LSB;
        }
        self.output_count |= bits;
        self.output_count = self.output_count.wrapping_add(1);
        HSDS_YIELD_BACKREF
    }

    fn yield_backref(&mut self, buffers: &mut [u8], out: &mut [u8], produced: &mut usize) -> u8 {
        let mut count = out.len() - *produced;
        if count > 0 {
            if (self.output_count as usize) < count {
                count = self.output_count as usize;
            }
            let mask = (1u16 << self.window_sz2).wrapping_sub(1);
            let win = self.input_buffer_size as usize;
            let neg_offset = self.output_index;
            for _ in 0..count {
                let src = self.head_index.wrapping_sub(neg_offset) & mask;
                let c = buffers[win + src as usize];
                out[*produced] = c;
                *produced += 1;
                buffers[win + (self.head_index & mask) as usize] = c;
                self.head_index = self.head_index.wrapping_add(1);
            }
            self.output_count = self.output_count.wrapping_sub(count as u16);
            if self.output_count == 0 {
                return HSDS_TAG_BIT;
            }
        }
        HSDS_YIELD_BACKREF
    }

    fn get_bits(&mut self, buffers: &[u8], count: u8) -> u16 {
        if count > 15 {
            return NO_BITS;
        }
        if self.input_size == 0 {
            // C: `bit_index < (1 << (count - 1))`. count is 1..15 from the state machine.
            if count == 0 {
                return NO_BITS;
            }
            let threshold = 1u32 << (count - 1);
            if u32::from(self.bit_index) < threshold {
                return NO_BITS;
            }
        }
        let mut accumulator: u16 = 0;
        for _ in 0..count {
            if self.bit_index == 0x00 {
                if self.input_size == 0 {
                    return NO_BITS;
                }
                self.current_byte = buffers[self.input_index as usize];
                self.input_index = self.input_index.wrapping_add(1);
                if self.input_index == self.input_size {
                    self.input_index = 0;
                    self.input_size = 0;
                }
                self.bit_index = 0x80;
            }
            accumulator <<= 1;
            if self.current_byte & self.bit_index != 0 {
                accumulator |= 0x01;
            }
            self.bit_index >>= 1;
        }
        accumulator
    }
}

/// Decoder input buffer has no free space (`HSDR_SINK_FULL`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SinkFull;

/// Owned decoder.
#[derive(Clone, Debug)]
pub struct Decoder {
    state: DecoderState,
    buffers: Vec<u8>,
}

impl Decoder {
    pub fn try_new(
        input_buffer_size: u16,
        window_sz2: u8,
        lookahead_sz2: u8,
    ) -> Result<Self, Error> {
        if !decoder_params_ok(input_buffer_size, window_sz2, lookahead_sz2) {
            return Err(Error::InvalidParams);
        }
        Ok(Self::from_params(
            input_buffer_size,
            window_sz2,
            lookahead_sz2,
        ))
    }

    /// Static configuration: input buffer 32, window 8, lookahead 4.
    pub fn from_static_config() -> Self {
        Self::from_params(
            STATIC_INPUT_BUFFER_SIZE,
            STATIC_WINDOW_BITS,
            STATIC_LOOKAHEAD_BITS,
        )
    }

    fn from_params(input_buffer_size: u16, window_sz2: u8, lookahead_sz2: u8) -> Self {
        let n = (1usize << window_sz2) + input_buffer_size as usize;
        let mut dec = Self {
            state: DecoderState::fresh(input_buffer_size, window_sz2, lookahead_sz2),
            buffers: vec![0; n],
        };
        dec.reset();
        dec
    }

    pub fn reset(&mut self) {
        self.state.reset(&mut self.buffers);
    }

    pub fn sink(&mut self, input: &[u8]) -> Result<usize, SinkFull> {
        self.state.sink(&mut self.buffers, input)
    }

    pub fn poll(&mut self, out: &mut [u8]) -> Result<crate::encoder::PollBytes, Error> {
        self.state.poll(&mut self.buffers, out)
    }

    pub fn finish(&self) -> bool {
        self.state.finish()
    }

    pub fn input_size(&self) -> u16 {
        self.state.input_size
    }

    pub fn input_index(&self) -> u16 {
        self.state.input_index
    }

    pub fn window_bits(&self) -> u8 {
        self.state.window_sz2
    }

    pub fn lookahead_bits(&self) -> u8 {
        self.state.lookahead_sz2
    }

    pub fn input_buffer_size(&self) -> u16 {
        self.state.input_buffer_size
    }
}
