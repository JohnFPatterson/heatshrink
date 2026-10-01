//! Encoder state machine, ported from `heatshrink_encoder.c`.

use crate::config::{
    params_ok, BACKREF_MARKER, LITERAL_MARKER, STATIC_LOOKAHEAD_BITS, STATIC_WINDOW_BITS,
};
use crate::error::Error;

const HSES_NOT_FULL: u8 = 0;
const HSES_FILLED: u8 = 1;
const HSES_SEARCH: u8 = 2;
const HSES_YIELD_TAG_BIT: u8 = 3;
const HSES_YIELD_LITERAL: u8 = 4;
const HSES_YIELD_BR_INDEX: u8 = 5;
const HSES_YIELD_BR_LENGTH: u8 = 6;
const HSES_SAVE_BACKLOG: u8 = 7;
const HSES_FLUSH_BITS: u8 = 8;
const HSES_DONE: u8 = 9;

const FLAG_IS_FINISHING: u8 = 0x01;
const MATCH_NOT_FOUND: u16 = u16::MAX;

/// Scalar encoder state. Buffers are owned by [`Encoder`] or by the FFI allocation.
#[derive(Clone, Debug)]
pub struct EncoderState {
    pub input_size: u16,
    pub match_scan_index: u16,
    pub match_length: u16,
    pub match_pos: u16,
    pub outgoing_bits: u16,
    pub outgoing_bits_count: u8,
    pub flags: u8,
    pub state: u8,
    pub current_byte: u8,
    pub bit_index: u8,
    pub window_sz2: u8,
    pub lookahead_sz2: u8,
}

impl EncoderState {
    pub fn fresh(window_sz2: u8, lookahead_sz2: u8) -> Self {
        Self {
            input_size: 0,
            match_scan_index: 0,
            match_length: 0,
            match_pos: 0,
            outgoing_bits: 0,
            outgoing_bits_count: 0,
            flags: 0,
            state: HSES_NOT_FULL,
            current_byte: 0,
            bit_index: 0x80,
            window_sz2,
            lookahead_sz2,
        }
    }

    pub fn input_buffer_size(&self) -> u16 {
        1u16 << self.window_sz2
    }

    fn lookahead_size(&self) -> u16 {
        1u16 << self.lookahead_sz2
    }

    fn is_finishing(&self) -> bool {
        self.flags & FLAG_IS_FINISHING != 0
    }

    /// Clear stream state and the sliding buffer. Does not touch the search index.
    pub fn reset(&mut self, buffer: &mut [u8]) {
        let buf_sz = 2usize << self.window_sz2;
        buffer[..buf_sz].fill(0);
        self.input_size = 0;
        self.state = HSES_NOT_FULL;
        self.match_scan_index = 0;
        self.flags = 0;
        self.bit_index = 0x80;
        self.current_byte = 0;
        self.match_length = 0;
        self.outgoing_bits = 0;
        self.outgoing_bits_count = 0;
    }

    pub fn sink(&mut self, buffer: &mut [u8], input: &[u8]) -> Result<usize, Error> {
        if self.is_finishing() || self.state != HSES_NOT_FULL {
            return Err(Error::Misuse);
        }
        let write_offset = self.input_buffer_size().wrapping_add(self.input_size);
        let ibs = self.input_buffer_size();
        let rem = ibs.wrapping_sub(self.input_size);
        let cp = std::cmp::min(rem as usize, input.len());
        let dst = write_offset as usize;
        buffer[dst..dst + cp].copy_from_slice(&input[..cp]);
        self.input_size = self.input_size.wrapping_add(cp as u16);
        if cp == rem as usize {
            self.state = HSES_FILLED;
        }
        Ok(cp)
    }

    pub fn poll(
        &mut self,
        buffer: &mut [u8],
        index: &mut [i16],
        out: &mut [u8],
    ) -> Result<PollBytes, Error> {
        if out.is_empty() {
            return Err(Error::EmptyOutput);
        }
        let mut produced = 0usize;
        loop {
            let in_state = self.state;
            match in_state {
                HSES_NOT_FULL => {
                    return Ok(PollBytes {
                        more: false,
                        n: produced,
                    })
                }
                HSES_FILLED => {
                    do_indexing(buffer, index, self.input_buffer_size(), self.input_size);
                    self.state = HSES_SEARCH;
                }
                HSES_SEARCH => {
                    self.state = self.step_search(buffer, index);
                }
                HSES_YIELD_TAG_BIT => {
                    self.state = self.yield_tag_bit(out, &mut produced);
                }
                HSES_YIELD_LITERAL => {
                    self.state = self.yield_literal(buffer, out, &mut produced);
                }
                HSES_YIELD_BR_INDEX => {
                    self.state = self.yield_br_index(out, &mut produced);
                }
                HSES_YIELD_BR_LENGTH => {
                    self.state = self.yield_br_length(out, &mut produced);
                }
                HSES_SAVE_BACKLOG => {
                    self.state = self.save_backlog_state(buffer);
                }
                // `heatshrink_encoder.c` has no `break` after FLUSH_BITS, so
                // control falls into the DONE arm and always returns EMPTY.
                HSES_FLUSH_BITS => {
                    self.state = self.flush_bits(out, &mut produced);
                    return Ok(PollBytes {
                        more: false,
                        n: produced,
                    });
                }
                HSES_DONE => {
                    return Ok(PollBytes {
                        more: false,
                        n: produced,
                    })
                }
                _ => return Err(Error::Misuse),
            }
            if self.state == in_state && produced == out.len() {
                return Ok(PollBytes {
                    more: true,
                    n: produced,
                });
            }
        }
    }

    pub fn finish(&mut self) -> bool {
        self.flags |= FLAG_IS_FINISHING;
        if self.state == HSES_NOT_FULL {
            self.state = HSES_FILLED;
        }
        self.state == HSES_DONE
    }

    fn step_search(&mut self, buffer: &[u8], index: &[i16]) -> u8 {
        let window_length = self.input_buffer_size();
        let lookahead_sz = self.lookahead_size();
        let msi = self.match_scan_index;
        let fin = self.is_finishing();
        let limit = if fin {
            self.input_size.wrapping_sub(1)
        } else {
            self.input_size.wrapping_sub(lookahead_sz)
        };
        if msi > limit {
            return if fin {
                HSES_FLUSH_BITS
            } else {
                HSES_SAVE_BACKLOG
            };
        }

        let input_offset = self.input_buffer_size();
        let end = input_offset.wrapping_add(msi);
        let start = end.wrapping_sub(window_length);
        let mut max_possible = lookahead_sz;
        if self.input_size.wrapping_sub(msi) < lookahead_sz {
            max_possible = self.input_size.wrapping_sub(msi);
        }
        let mut match_length = 0u16;
        let match_pos = find_longest_match(
            self,
            buffer,
            index,
            start,
            end,
            max_possible,
            &mut match_length,
        );
        if match_pos == MATCH_NOT_FOUND {
            self.match_scan_index = self.match_scan_index.wrapping_add(1);
            self.match_length = 0;
            HSES_YIELD_TAG_BIT
        } else {
            self.match_pos = match_pos;
            self.match_length = match_length;
            HSES_YIELD_TAG_BIT
        }
    }

    fn yield_tag_bit(&mut self, out: &mut [u8], produced: &mut usize) -> u8 {
        if can_take(out, *produced) {
            if self.match_length == 0 {
                self.push_bits(1, LITERAL_MARKER, out, produced);
                HSES_YIELD_LITERAL
            } else {
                self.push_bits(1, BACKREF_MARKER, out, produced);
                self.outgoing_bits = self.match_pos.wrapping_sub(1);
                self.outgoing_bits_count = self.window_sz2;
                HSES_YIELD_BR_INDEX
            }
        } else {
            HSES_YIELD_TAG_BIT
        }
    }

    fn yield_literal(&mut self, buffer: &[u8], out: &mut [u8], produced: &mut usize) -> u8 {
        if can_take(out, *produced) {
            let processed_offset = self.match_scan_index.wrapping_sub(1);
            let input_offset = self.input_buffer_size().wrapping_add(processed_offset);
            let c = buffer[input_offset as usize];
            self.push_bits(8, c, out, produced);
            HSES_SEARCH
        } else {
            HSES_YIELD_LITERAL
        }
    }

    fn yield_br_index(&mut self, out: &mut [u8], produced: &mut usize) -> u8 {
        if can_take(out, *produced) {
            if self.push_outgoing_bits(out, produced) > 0 {
                HSES_YIELD_BR_INDEX
            } else {
                self.outgoing_bits = self.match_length.wrapping_sub(1);
                self.outgoing_bits_count = self.lookahead_sz2;
                HSES_YIELD_BR_LENGTH
            }
        } else {
            HSES_YIELD_BR_INDEX
        }
    }

    fn yield_br_length(&mut self, out: &mut [u8], produced: &mut usize) -> u8 {
        if can_take(out, *produced) {
            if self.push_outgoing_bits(out, produced) > 0 {
                HSES_YIELD_BR_LENGTH
            } else {
                self.match_scan_index = self.match_scan_index.wrapping_add(self.match_length);
                self.match_length = 0;
                HSES_SEARCH
            }
        } else {
            HSES_YIELD_BR_LENGTH
        }
    }

    fn save_backlog_state(&mut self, buffer: &mut [u8]) -> u8 {
        let input_buf_sz = self.input_buffer_size();
        let msi = self.match_scan_index;
        let rem = input_buf_sz.wrapping_sub(msi);
        let shift_sz = input_buf_sz.wrapping_add(rem) as usize;
        let src = input_buf_sz.wrapping_sub(rem) as usize;
        buffer.copy_within(src..src + shift_sz, 0);
        self.match_scan_index = 0;
        self.input_size = self.input_size.wrapping_sub(input_buf_sz.wrapping_sub(rem));
        HSES_NOT_FULL
    }

    fn flush_bits(&mut self, out: &mut [u8], produced: &mut usize) -> u8 {
        if self.bit_index == 0x80 {
            HSES_DONE
        } else if can_take(out, *produced) {
            out[*produced] = self.current_byte;
            *produced += 1;
            HSES_DONE
        } else {
            HSES_FLUSH_BITS
        }
    }

    fn push_outgoing_bits(&mut self, out: &mut [u8], produced: &mut usize) -> u8 {
        let (count, bits) = if self.outgoing_bits_count > 8 {
            let count = 8u8;
            let bits = (self.outgoing_bits >> (self.outgoing_bits_count - 8)) as u8;
            (count, bits)
        } else {
            (self.outgoing_bits_count, self.outgoing_bits as u8)
        };
        if count > 0 {
            self.push_bits(count, bits, out, produced);
            self.outgoing_bits_count -= count;
        }
        count
    }

    fn push_bits(&mut self, count: u8, bits: u8, out: &mut [u8], produced: &mut usize) {
        if count == 8 && self.bit_index == 0x80 {
            out[*produced] = bits;
            *produced += 1;
            return;
        }
        let mut i = i32::from(count) - 1;
        while i >= 0 {
            let bit = (bits & (1u8 << i)) != 0;
            if bit {
                self.current_byte |= self.bit_index;
            }
            self.bit_index >>= 1;
            if self.bit_index == 0x00 {
                self.bit_index = 0x80;
                out[*produced] = self.current_byte;
                *produced += 1;
                self.current_byte = 0x00;
            }
            i -= 1;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PollBytes {
    pub more: bool,
    pub n: usize,
}

fn can_take(out: &[u8], produced: usize) -> bool {
    produced < out.len()
}

fn do_indexing(buffer: &[u8], index: &mut [i16], input_offset: u16, input_size: u16) {
    let mut last = [-1i16; 256];
    let end = input_offset.wrapping_add(input_size);
    for i in 0..end {
        let v = buffer[i as usize];
        let lv = last[v as usize];
        index[i as usize] = lv;
        // `last[v] = i` stores a uint16 in an int16. GCC truncates.
        last[v as usize] = i as i16;
    }
}

fn find_longest_match(
    state: &EncoderState,
    buffer: &[u8],
    index: &[i16],
    start: u16,
    end: u16,
    maxlen: u16,
    match_length: &mut u16,
) -> u16 {
    let mut match_maxlen: u16 = 0;
    let mut match_index: u16 = MATCH_NOT_FOUND;
    let mut pos: i16 = index[end as usize];
    let start_i = start as i16;
    while i32::from(pos) - i32::from(start_i) >= 0 {
        let pos_us = pos as u16 as usize;
        let needle = end as usize;
        if buffer[pos_us + match_maxlen as usize] != buffer[needle + match_maxlen as usize] {
            pos = index[pos_us];
            continue;
        }
        let mut len: u16 = 1;
        while len < maxlen {
            if buffer[pos_us + len as usize] != buffer[needle + len as usize] {
                break;
            }
            len = len.wrapping_add(1);
        }
        if len > match_maxlen {
            match_maxlen = len;
            match_index = pos as u16;
            if len == maxlen {
                break;
            }
        }
        pos = index[pos_us];
    }
    let break_even = (1u16 + u16::from(state.window_sz2) + u16::from(state.lookahead_sz2)) / 8;
    if match_maxlen > break_even {
        *match_length = match_maxlen;
        end.wrapping_sub(match_index)
    } else {
        MATCH_NOT_FOUND
    }
}

/// Owned encoder. Window and lookahead are chosen at construction, which is
/// the dynamic API. [`Encoder::from_static_config`] uses the static-build constants.
#[derive(Clone, Debug)]
pub struct Encoder {
    state: EncoderState,
    buffer: Vec<u8>,
    index: Vec<i16>,
}

impl Encoder {
    pub fn try_new(window_sz2: u8, lookahead_sz2: u8) -> Result<Self, Error> {
        if !params_ok(window_sz2, lookahead_sz2) {
            return Err(Error::InvalidParams);
        }
        Ok(Self::from_params(window_sz2, lookahead_sz2))
    }

    /// Static configuration from `heatshrink_config.h` (`window` 8, `lookahead` 4).
    pub fn from_static_config() -> Self {
        Self::from_params(STATIC_WINDOW_BITS, STATIC_LOOKAHEAD_BITS)
    }

    fn from_params(window_sz2: u8, lookahead_sz2: u8) -> Self {
        let buf_sz = 2usize << window_sz2;
        let mut enc = Self {
            state: EncoderState::fresh(window_sz2, lookahead_sz2),
            buffer: vec![0; buf_sz],
            index: vec![0; buf_sz],
        };
        enc.reset();
        enc
    }

    pub fn reset(&mut self) {
        self.state.reset(&mut self.buffer);
    }

    pub fn sink(&mut self, input: &[u8]) -> Result<usize, Error> {
        self.state.sink(&mut self.buffer, input)
    }

    pub fn poll(&mut self, out: &mut [u8]) -> Result<PollBytes, Error> {
        self.state.poll(&mut self.buffer, &mut self.index, out)
    }

    /// `true` when encoding is finished (`HSER_FINISH_DONE`).
    pub fn finish(&mut self) -> bool {
        self.state.finish()
    }

    pub fn window_bits(&self) -> u8 {
        self.state.window_sz2
    }

    pub fn lookahead_bits(&self) -> u8 {
        self.state.lookahead_sz2
    }
}
