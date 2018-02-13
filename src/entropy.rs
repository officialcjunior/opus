use bitstream::bitread::*;
use maths::*;
little_endian_reader!{ ReverseBitReadLE }

impl<'a> ReverseBitReadLE<'a> {
    #[inline(always)]
    fn fill(&self, count: usize) -> u64 {
        let len = self.buffer.len();
        let end = len - self.index;
        let start = end.saturating_sub(count);
        let mut v = 0;

        for b in self.buffer[start..end].iter() {
            v = v << 8 | *b as u64;
        }

        v
    }
}

impl<'a> BitReadFill for ReverseBitReadLE<'a> {
    // TODO: check if we can safely read in batches of 4 or 8
    #[inline(always)]
    fn can_refill(&self) -> bool {
        self.index <= self.buffer.len()
    }
    #[inline(always)]
    fn fill32(&self) -> u64 {
        self.fill(4)
    }
    #[inline(always)]
    fn fill64(&self) -> u64 {
        self.fill(8)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn reverse_bitread() {
        let buf = &[197, 105, 76, 120, 136, 74, 169, 50, 225, 8, 231, 211, 227, 151, 186, 58, 173, 139];

        let mut r = ReverseBitReadLE::new(buf);

        assert_eq!(r.get_bits_32(3), 3);
        assert_eq!(r.get_bits_32(3), 1);
        assert_eq!(r.get_bits_32(3), 6);
        assert_eq!(r.get_bits_32(3), 6);
        assert_eq!(r.get_bits_32(3), 2);
        assert_eq!(r.get_bits_32(3), 5);
        assert_eq!(r.get_bits_32(3), 6);
        assert_eq!(r.get_bits_32(3), 1);
        assert_eq!(r.get_bits_32(2), 2);
        assert_eq!(r.get_bits_32(2), 2);
        assert_eq!(r.get_bits_32(3), 3);
        assert_eq!(r.get_bits_32(3), 7);
        assert_eq!(r.get_bits_32(3), 5);
        assert_eq!(r.get_bits_32(3), 4);
        assert_eq!(r.get_bits_32(2), 3);
        assert_eq!(r.get_bits_32(2), 0);
        assert_eq!(r.get_bits_32(3), 6);
        assert_eq!(r.get_bits_32(3), 7);
        assert_eq!(r.get_bits_32(3), 4);
        assert_eq!(r.get_bits_32(3), 6);
        assert_eq!(r.get_bits_32(3), 7);
        assert_eq!(r.get_bits_32(3), 4);
        assert_eq!(r.get_bits_32(3), 3);
        assert_eq!(r.get_bits_32(3), 4);
        assert_eq!(r.get_bits_32(3), 0);
        assert_eq!(r.get_bits_32(3), 2);
        assert_eq!(r.get_bits_32(3), 0);
        assert_eq!(r.get_bits_32(3), 7);
        assert_eq!(r.get_bits_32(3), 2);
        assert_eq!(r.get_bits_32(3), 6);
        assert_eq!(r.get_bits_32(3), 4);
        assert_eq!(r.get_bits_32(3), 4);
        assert_eq!(r.get_bits_32(3), 2);
        assert_eq!(r.get_bits_32(3), 5);
        assert_eq!(r.get_bits_32(3), 2);
        assert_eq!(r.get_bits_32(3), 2);
        assert_eq!(r.get_bits_32(3), 0);
        assert_eq!(r.get_bits_32(3), 1);
        assert_eq!(r.get_bits_32(3), 2);
        assert_eq!(r.get_bits_32(3), 4);
        assert_eq!(r.get_bits_32(4), 7);
        assert_eq!(r.get_bits_32(4), 12);
        assert_eq!(r.get_bits_32(19), 284308);
    }
}

/// Opus Range Decoder
///
/// See [rfc6716 section 4.1](https://tools.ietf.org/html/rfc6716#section-4.1)
#[derive(Debug)]
pub struct RangeDecoder<'a> {
    bits: BitReadBE<'a>,
    revs: ReverseBitReadLE<'a>,
    range: usize,
    value: usize,
    total: usize,
}

pub struct ICDFContext {
    pub total: usize,
    pub dist: &'static [usize],
}

const SYM_BITS: usize = 8;
const SYM_MAX: usize = (1 << SYM_BITS) - 1;

const CODE_BITS: usize = 32;
const CODE_SHIFT: usize = CODE_BITS - SYM_BITS - 1;
const CODE_TOP: usize = 1 << (CODE_BITS - 1);
const CODE_BOT: usize = CODE_TOP >> SYM_BITS;
const CODE_EXTRA: usize =  (CODE_BITS - 2) % SYM_BITS + 1;

impl<'a> RangeDecoder<'a> {
    fn normalize(&mut self) {
        while self.range <= CODE_BOT {
            let v = self.bits.get_bits_32(SYM_BITS) as usize ^ SYM_MAX;
            self.value = ((self.value << SYM_BITS) | v) & (CODE_TOP - 1);
            self.range <<= SYM_BITS;
            self.total += SYM_BITS;
        }
    }

    pub fn new(buf: &'a [u8]) -> Self {
        let mut bits = BitReadBE::new(buf);
        let value = 127 - bits.get_bits_32(7) as usize;

        let mut r = RangeDecoder {
            bits: bits,
            revs: ReverseBitReadLE::new(buf),
            range: 128,
            value: value,
            total: CODE_BITS + 1,
        };

        r.normalize();

        r
    }

    fn update(&mut self, scale: usize, low: usize, high: usize, total: usize) {
        let s = scale * (total - high);
        // println!("update {} {} {} {} {} -> {}", scale, low, high, total, s, self.value);
        self.value -= s;
        self.range = if low != 0 {
            scale * (high - low)
        } else {
            self.range - s
        };


        assert_ne!(self.range, 0);

        self.normalize();
    }

    fn get_scale_symbol(&self, total: usize) -> (usize, usize) {
        let scale = self.range / total;
        let k = total - (self.value / scale + 1).min(total);

        (scale, k)
    }

    pub fn decode_logp(&mut self, logp: usize) -> bool {
        let scale = self.range >> logp;

        // println!("p2 scale {} bits {}", scale, logp);
        let k  = if scale > self.value {
            self.range = scale;
            true
        } else {
            self.range -= scale;
            self.value -= scale;
            false
        };

        self.normalize();

        k
    }

    pub fn decode_icdf(&mut self, icdf: &ICDFContext) -> usize {
        let total = icdf.total;
        let dist = icdf.dist;
        let (scale, sym) = self.get_scale_symbol(total);
        let k = dist.iter().position(|v| *v > sym).unwrap();
        let high = dist[k];
        let low = if k > 0 { dist[k - 1] } else { 0 };
        // println!("{} {} decode to {}", scale, sym, k);
        self.update(scale, low, high, total);

        k
    }

    #[inline(always)]
    pub fn tell(&self) -> usize {
        self.total - self.range.ilog()
    }

    #[inline(always)]
    pub fn tell_frac(&self) -> usize {
        let mut lg = self.range.ilog();
        let mut rq15 = self.range >> (lg - 16);

        for _ in 0..3 {
            rq15 = (rq15 * rq15) >> (lg - 16);
            let lastbit = rq15 >> 16;
            lg = lg * 2 + lastbit;
            if lastbit != 0 {
                rq15 >>= 1;
            }
        }

        self.total * 8 - lg
    }
}

trait CeltOnly {
    fn rawbits(&mut self, len: usize) -> usize;
    fn decode_uniform(&mut self, len: usize) -> usize;
}

const UNI_BITS: usize = 8;

impl<'a> CeltOnly for RangeDecoder<'a> {
    fn rawbits(&mut self, len: usize) -> usize {
        self.revs.get_bits_32(len) as usize
    }

    fn decode_uniform(&mut self, len: usize) -> usize {
        let bits = (len - 1).ilog();

        let total = if bits > UNI_BITS {
            ((len - 1) >> (bits - UNI_BITS)) + 1
        } else {
            len
        };

        let (scale, k) = self.get_scale_symbol(total);

        self.update(scale, k, k + 1, total);

        if bits > UNI_BITS {
            k << (bits - UNI_BITS) | self.rawbits(bits - UNI_BITS)
        } else {
            k
        }
    }
}
