use std::ops::Range;
use std::mem;

use entropy::*;
use maths::*;
use packet::*;

const SHORT_BLOCKSIZE: usize = 120;
const MAX_BANDS: usize = 21;
const MIN_PERIOD: usize = 15;

const SPREAD_NONE: usize = 0;
const SPREAD_LIGHT: usize = 1;
const SPREAD_NORMAL: usize = 2;
const SPREAD_AGGRESSIVE: usize = 3;

#[derive(Debug, Default)]
struct PostFilter {
    period: usize,
    period_new: usize,
    period_old: usize,

    gains: [f32; 3],
    gains_new: [f32; 3],
    gains_old: [f32; 3],
}

#[derive(Debug)]
struct CeltFrame {
    pf: PostFilter,
    energy: [f32; MAX_BANDS],
    prev_energy: [f32; MAX_BANDS],
    collapse_masks: [u8; MAX_BANDS],

    buf: Vec<f32>, // TODO: replace with an array once const-generics

    deemph_coeff: f32,
}

impl Default for CeltFrame {
    fn default() -> Self {
        CeltFrame {
            pf: Default::default(),
            energy: Default::default(),
            prev_energy: Default::default(),
            collapse_masks: Default::default(),

            buf: vec![0f32; 2048],

            deemph_coeff: 0f32,
        }
    }
}

#[derive(Debug)]
pub struct Celt {
    stereo: bool,
    stereo_pkt: bool,
    bits: usize,
    lm: usize, // aka duration in mdct blocks
    band: Range<usize>,
    frames: [CeltFrame; 2],
    spread: usize,

    fine_bits: [usize; MAX_BANDS],
    fine_priority: [usize; MAX_BANDS],
    pulses: [i32; MAX_BANDS],
    tf_change: [i8; MAX_BANDS],

    anticollapse_bit: usize,
    blocks: usize,
    blocksize: usize,
}

const POSTFILTER_TAPS: &[&[f32]] = &[
    &[0.3066406250, 0.2170410156, 0.1296386719],
    &[0.4638671875, 0.2680664062, 0.0],
    &[0.7998046875, 0.1000976562, 0.0],
];

const TAPSET: &ICDFContext = &ICDFContext {
    total: 4,
    dist: &[2, 3, 4],
};

const ALPHA_COEF: &[f32] = &[
    29440.0 / 32768.0,
    26112.0 / 32768.0,
    21248.0 / 32768.0,
    16384.0 / 32768.0,
];

const BETA_COEF: &[f32] = &[
    1.0 - 30147.0 / 32768.0,
    1.0 - 22282.0 / 32768.0,
    1.0 - 12124.0 / 32768.0,
    1.0 - 6554.0 / 32768.0,
];

// TODO: make it a &[&[(u8, u8)]] if it makes no speed difference
const COARSE_ENERGY_INTRA: &[&[u8]] = &[
    // 120-samples
    &[
        24, 179, 48, 138, 54, 135, 54, 132, 53, 134, 56, 133, 55, 132, 55, 132, 61, 114, 70, 96,
        74, 88, 75, 88, 87, 74, 89, 66, 91, 67, 100, 59, 108, 50, 120, 40, 122, 37, 97, 43, 78, 50,
    ],
    // 240-samples
    &[
        23, 178, 54, 115, 63, 102, 66, 98, 69, 99, 74, 89, 71, 91, 73, 91, 78, 89, 86, 80, 92, 66,
        93, 64, 102, 59, 103, 60, 104, 60, 117, 52, 123, 44, 138, 35, 133, 31, 97, 38, 77, 45,
    ],
    // 480-samples
    &[
        21, 178, 59, 110, 71, 86, 75, 85, 84, 83, 91, 66, 88, 73, 87, 72, 92, 75, 98, 72, 105, 58,
        107, 54, 115, 52, 114, 55, 112, 56, 129, 51, 132, 40, 150, 33, 140, 29, 98, 35, 77, 42,
    ],
    // 960-samples
    &[
        22, 178, 63, 114, 74, 82, 84, 83, 92, 82, 103, 62, 96, 72, 96, 67, 101, 73, 107, 72, 113,
        55, 118, 52, 125, 52, 118, 52, 117, 55, 135, 49, 137, 39, 157, 32, 145, 29, 97, 33, 77, 40,
    ],
];

const COARSE_ENERGY_INTER: &[&[u8]] = &[
    // 120-samples
    &[
        72, 127, 65, 129, 66, 128, 65, 128, 64, 128, 62, 128, 64, 128, 64, 128, 92, 78, 92, 79, 92,
        78, 90, 79, 116, 41, 115, 40, 114, 40, 132, 26, 132, 26, 145, 17, 161, 12, 176, 10, 177,
        11,
    ],
    // 240-samples
    &[
        83, 78, 84, 81, 88, 75, 86, 74, 87, 71, 90, 73, 93, 74, 93, 74, 109, 40, 114, 36, 117, 34,
        117, 34, 143, 17, 145, 18, 146, 19, 162, 12, 165, 10, 178, 7, 189, 6, 190, 8, 177, 9,
    ],
    // 480-samples
    &[
        61, 90, 93, 60, 105, 42, 107, 41, 110, 45, 116, 38, 113, 38, 112, 38, 124, 26, 132, 27,
        136, 19, 140, 20, 155, 14, 159, 16, 158, 18, 170, 13, 177, 10, 187, 8, 192, 6, 175, 9, 159,
        10,
    ],
    // 960-samples
    &[
        42, 121, 96, 66, 108, 43, 111, 40, 117, 44, 123, 32, 120, 36, 119, 33, 127, 33, 134, 34,
        139, 21, 147, 23, 152, 20, 158, 25, 154, 26, 166, 21, 173, 16, 184, 13, 184, 10, 150, 13,
        139, 15,
    ],
];

const STATIC_CAPS: &[&[&[u8]]] = &[
    // 120-sample
    &[
        &[224, 224, 224, 224, 224, 224, 224, 224, 160, 160,
         160, 160, 185, 185, 185, 178, 178, 168, 134,  61,  37],
        &[224, 224, 224, 224, 224, 224, 224, 224, 240, 240,
         240, 240, 207, 207, 207, 198, 198, 183, 144,  66,  40],
    ],
    // 240-sample
    &[
        &[160, 160, 160, 160, 160, 160, 160, 160, 185, 185,
         185, 185, 193, 193, 193, 183, 183, 172, 138,  64,  38],
        &[240, 240, 240, 240, 240, 240, 240, 240, 207, 207,
         207, 207, 204, 204, 204, 193, 193, 180, 143,  66,  40],
    ],
    // 480-sample
    &[
        &[185, 185, 185, 185, 185, 185, 185, 185, 193, 193,
         193, 193, 193, 193, 193, 183, 183, 172, 138,  65,  39],
        &[207, 207, 207, 207, 207, 207, 207, 207, 204, 204,
         204, 204, 201, 201, 201, 188, 188, 176, 141,  66,  40],
    ],
    // 960-sample
    &[
        &[193, 193, 193, 193, 193, 193, 193, 193, 193, 193,
         193, 193, 194, 194, 194, 184, 184, 173, 139,  65,  39],
        &[204, 204, 204, 204, 204, 204, 204, 204, 201, 201,
         201, 201, 198, 198, 198, 187, 187, 175, 140,  66,  40]
    ],
];


const FREQ_RANGE: &[u8] = &[
    1,  1,  1,  1,  1,  1,  1,  1,  2,  2,  2,  2,  4,  4,  4,  6,  6,  8, 12, 18, 22
];


const MODEL_ENERGY_SMALL: &ICDFContext = &ICDFContext {
    total: 4,
    dist: &[2, 3, 4],
};

const TF_SELECT: &[[[[i8;2];2];2]] = &[
    [
        [
            [0, -1], [0, -1]
        ],
        [
            [0, -1], [0, -1]
        ],
    ],
    [
        [
            [0, -1], [0, -2]
        ],
        [
            [1, 0], [1, -1]
        ],
    ],
    [
        [
            [0, -2], [0, -3]
        ],
        [
            [2, 0], [1, -1]
        ],
    ],
    [
        [
            [0, -2], [0, -3]
        ],
        [
            [3, 0], [1, -1]
        ],
    ],
];

const MODEL_SPREAD: &ICDFContext = &ICDFContext {
    total: 32,
    dist: &[7, 9, 30, 32]
};


const ALLOC_TRIM: &ICDFContext = &ICDFContext {
    total: 128,
    dist: &[2,   4,   9,  19,  41,  87, 109, 119, 124, 126, 128]
};

const LOG2_FRAC: &[u8] = &[
    0, 8, 13, 16, 19, 21, 23, 24, 26, 27, 28, 29, 30, 31, 32, 32, 33, 34, 34, 35, 36, 36, 37, 37
];

const STATIC_ALLOC: &[[u8; 21]; 11] = &[  /* 1/32 bit/sample */
    [   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0 ],
    [  90,  80,  75,  69,  63,  56,  49,  40,  34,  29,  20,  18,  10,   0,   0,   0,   0,   0,   0,   0,   0 ],
    [ 110, 100,  90,  84,  78,  71,  65,  58,  51,  45,  39,  32,  26,  20,  12,   0,   0,   0,   0,   0,   0 ],
    [ 118, 110, 103,  93,  86,  80,  75,  70,  65,  59,  53,  47,  40,  31,  23,  15,   4,   0,   0,   0,   0 ],
    [ 126, 119, 112, 104,  95,  89,  83,  78,  72,  66,  60,  54,  47,  39,  32,  25,  17,  12,   1,   0,   0 ],
    [ 134, 127, 120, 114, 103,  97,  91,  85,  78,  72,  66,  60,  54,  47,  41,  35,  29,  23,  16,  10,   1 ],
    [ 144, 137, 130, 124, 113, 107, 101,  95,  88,  82,  76,  70,  64,  57,  51,  45,  39,  33,  26,  15,   1 ],
    [ 152, 145, 138, 132, 123, 117, 111, 105,  98,  92,  86,  80,  74,  67,  61,  55,  49,  43,  36,  20,   1 ],
    [ 162, 155, 148, 142, 133, 127, 121, 115, 108, 102,  96,  90,  84,  77,  71,  65,  59,  53,  46,  30,   1 ],
    [ 172, 165, 158, 152, 143, 137, 131, 125, 118, 112, 106, 100,  94,  87,  81,  75,  69,  63,  56,  45,  20 ],
    [ 200, 200, 200, 200, 200, 200, 200, 200, 198, 193, 188, 183, 178, 173, 168, 163, 158, 153, 148, 129, 104 ]
];

impl Celt {
    pub fn new(stereo: bool) -> Self {
        let frames = Default::default();
        Celt {
            stereo,
            stereo_pkt: false,
            bits: 0,
            lm: 0,
            frames,
            band: 0..MAX_BANDS,
            spread: SPREAD_NORMAL,
            fine_bits: Default::default(),
            fine_priority: Default::default(),
            pulses: Default::default(),
            tf_change: Default::default(),
            anticollapse_bit: 0,
            blocks: 0,
            blocksize: 0,
        }
    }

    pub fn setup(&mut self, pkt: &Packet) {
        self.stereo_pkt = pkt.stereo;
    }

    fn reset_gains(&mut self) {
        self.frames[0].pf.gains_new = [0.0; 3];
        self.frames[1].pf.gains_new = [0.0; 3];
    }

    fn parse_postfilter(&mut self, rd: &mut RangeDecoder) {
        if rd.decode_logp(1) {
            let octave = rd.decode_uniform(6);
            let period = (16 << octave) + rd.rawbits(4 + octave) - 1;
            let gain_bits = rd.rawbits(3) + 1;
            let gain = gain_bits as f32 * 0.09375;

            let tapset = if rd.available() >= 2 {
                rd.decode_icdf(TAPSET)
            } else {
                0
            };

            println!(
                "postfilter: octave {}, period {}, gain {}, tapset {}",
                octave, period, gain, tapset
            );
            let taps = POSTFILTER_TAPS[tapset];
            for frame in self.frames.iter_mut() {
                frame.pf.period_new = period.max(MIN_PERIOD);
                frame.pf.gains_new = [taps[0] * gain, taps[1] * gain, taps[2] * gain];
            }
        } else {
            println!("postfilter: no");
        }
    }

    fn decode_coarse_energy(&mut self, rd: &mut RangeDecoder, band: Range<usize>) {
        let (alpha, beta, model) = if rd.available() > 3 && rd.decode_logp(3) {
            (
                0f32,
                1f32 - 4915f32 / 32768f32,
                COARSE_ENERGY_INTRA[self.lm],
            )
        } else {
            (
                ALPHA_COEF[self.lm],
                BETA_COEF[self.lm],
                COARSE_ENERGY_INTER[self.lm],
            )
        };

        println!("model {:.6} {:.6}", alpha, beta);

        let mut prev = [0f32; 2];
        let frames = &mut self.frames;
        for i in 0..MAX_BANDS {
            let mut coarse_energy_band = |j| {
                let f: &mut CeltFrame = &mut frames[j];
                let en = &mut f.energy[i];
                if i < band.start || i >= band.end {
                    *en = 0.0
                } else {
                    let available = rd.available();
                    println!("available {}", available);
                    let value = if available >= 15 {
                        let k = i.min(20) << 1;
                        let v = rd
                            .decode_laplace((model[k] as usize) << 7, (model[k + 1] as isize) << 6);
                        println!("decode_laplace {:.6} <- {} {}", v, i, k);
                        v
                    } else if available >= 1 {
                        let v = rd.decode_icdf(MODEL_ENERGY_SMALL) as isize;
                        (v >> 1) ^ -(v & 1)
                    } else {
                        -1
                    } as f32;

                    println!("energy {}/{} {:.6} * {:.6} + {:.6} + {:.6}", i, j, *en, alpha, prev[j], value);
                    *en = en.max(-9f32) * alpha + prev[j] + value;
                    prev[j] += beta * value;
                }
            };

            coarse_energy_band(0);
            if self.stereo_pkt {
                coarse_energy_band(1);
            }
        }
        /*
        self.frames.iter_mut().for_each(|f| {
            let mut energy = f.energy.iter_mut().enumerate();
            let mut prev = 0f32;

            energy.by_ref().take(band.start).for_each(|(_, en)| {
                *en = 0f32;
            });

            energy.by_ref().take(band.end - band.start).for_each(|(i, en)| {
                let available = rd.available();
                let value = if available >= 15 {
                    let k = i.min(20) << 1;
                    let v = rd.decode_laplace((model[k] as usize) << 7, (model[k + 1] as isize) << 6)
                    println!("decode_laplace {} <- {} {}", v, i, k);
                    v
                } else if available >= 1 {
                    let v = rd.decode_icdf(MODEL_ENERGY_SMALL) as isize;
                    (v >> 1) ^ - (v & 1)
                } else {
                    -1
                } as f32;

                *en = en.max(-9f32) * alpha + prev + value;
                prev += beta * value;
            });

            energy.by_ref().for_each(|(_, en)| {
                *en = 0.0;
            });
        });
*/
        println!("{:#.6?}", &frames[0].energy[..]);
        println!("{:#.6?}", &frames[1].energy[..]);
    }

    fn decode_tf_changes(&mut self, rd: &mut RangeDecoder, band: Range<usize>, transient: bool) {
        let mut tf_changed = [false; MAX_BANDS];
        let bits = if transient { (2, 4) } else { (4, 5) };
        let mut available = rd.available();

        let tf_select = TF_SELECT[self.lm][transient as usize];

        let select_bit = self.lm != 0 && available > bits.0;
        println!("select_bit {} {}", select_bit, available);

        let mut field_bits = bits.0;
        let mut diff = false;
        let mut changed = false;
        for (i, tf_change) in tf_changed[band.clone()].iter_mut().enumerate() {
            if available > field_bits + select_bit as usize {
                diff ^= rd.decode_logp(field_bits);
                println!("band {} bits {} {}", i, field_bits, diff);
                available = rd.available();
                changed |= diff;
            }

            *tf_change = diff;
            field_bits = bits.1;
        }

        let select = if select_bit && tf_select[0][changed as usize] != tf_select[1][changed as usize] {
            rd.decode_logp(1)
        } else {
            false
        };
        {
            let tf_change = self.tf_change[band.clone()].iter_mut();

            for (tf, &changed) in tf_change.zip(tf_changed[band.clone()].iter()) {
                *tf = tf_select[select as usize][changed as usize];
            }
        }
        println!("tf_change {:#?}", &self.tf_change[band]);
    }

    fn decode_fine_energy(&mut self, rd: &mut RangeDecoder, band: Range<usize>) {
        self.frames.iter_mut().for_each(|f| {
            let energy = f.energy.iter_mut().enumerate();
        });
    }

    fn decode_allocation(&mut self, rd: &mut RangeDecoder, band: Range<usize>) {
        let mut caps: [i32; MAX_BANDS] = unsafe { mem::uninitialized() };
        let mut threshold = [0; MAX_BANDS];
        let mut trim_offset = [0; MAX_BANDS];
        let mut boost = [0; MAX_BANDS];
        let scale = self.lm + self.stereo_pkt as usize;
        let mut skip_startband = band.start;

        let spread = if rd.available() > 4 {
            rd.decode_icdf(MODEL_SPREAD)
        } else {
            SPREAD_NORMAL
        };

        let static_caps = &STATIC_CAPS[self.lm][self.stereo_pkt as usize];

        caps.iter_mut().zip(static_caps.iter().zip(FREQ_RANGE.iter()))
            .for_each(|(cap, (&static_cap, &freq_range)) | {
            *cap = (static_cap as i32 + 64) * (freq_range as i32) << scale >> 2;
        });

        println!("caps {:#?}", &caps[..]);

        let mut dynalloc = 6;
        let mut boost_size = 0;

        println!("consumed {}", rd.tell_frac());

        for i in band.clone() {
            let quanta = FREQ_RANGE[i] << scale;
            let quanta = (quanta << 3).min(quanta.max(6 << 3)) as i32;
            let mut band_dynalloc = dynalloc;
            while (band_dynalloc << 3) + boost_size < rd.available_frac() && boost[i] < caps[i] {
                let add = rd.decode_logp(band_dynalloc);
                if !add {
                    break;
                }
                boost[i] += quanta;
                boost_size += quanta as usize;
                band_dynalloc = 1;
            }

            if boost[i] != 0 && dynalloc > 2 {
                dynalloc -= 1;
            }
        }

        let alloc_trim = if rd.available_frac() > boost_size + (6 << 3) {
            rd.decode_icdf(ALLOC_TRIM)
        } else {
            5
        } as i32;

        println!("alloc_trim {}", alloc_trim);

        let mut available = rd.available_frac() - 1;
        self.anticollapse_bit = if self.blocks > 1 && self.lm >= 2 && available >= (self.lm + 2) << 3 {
            available -= 1 << 3;
            1 << 3
        } else {
            0
        };

        println!("anticollapse_bit {}", self.anticollapse_bit);

        let skip_bit = if available >= 1 << 3 {
            available -= 1 << 3;
            1 << 3
        } else {
            0
        };

        println!("skip_bit {}", skip_bit);


        let (intensity_stereo_bit, dual_stereo_bit) = if self.stereo_pkt {
            let intensity_stereo = LOG2_FRAC[band.end - band.start] as usize;
            if intensity_stereo <= available {
                available -= intensity_stereo;
                let dual_stereo = if available >= 1 << 3 {
                    available -= 1 << 3;
                    1 << 3
                } else {
                    0
                };
                (intensity_stereo, dual_stereo)
            } else {
                (0, 0)
            }
        } else {
            (0, 0)
        };

        println!("intensity_stereo_bit {}", intensity_stereo_bit);

        for i in band.clone() {
            let trim = alloc_trim - (5 + self.lm) as i32;
            let range = FREQ_RANGE[i] as i32 * (band.end - i - 1) as i32;
            let lm = self.lm + 3;
            let scale = lm as i32 + self.stereo_pkt as i32;
            let stereo_threshold = (self.stereo_pkt as i32) << 8;

            threshold[i] = ((3 * FREQ_RANGE[i] as i32) << lm >> 4).max(stereo_threshold);

            trim_offset[i] = trim * (range << scale) >> 6;

            if FREQ_RANGE[i] << self.lm == 1 {
                trim_offset[i] -= stereo_threshold;
            }

            println!("trim_offset {} {}", i, trim_offset[i]);
        }


        const CELT_VECTOR: usize = 11;
        let coded_channel_bits = (self.stereo_pkt as i32 + 1) << 3;

        let mut low = 1;
        let mut high = CELT_VECTOR - 1;
        while low <= high {
            let center = (low + high) / 2;
            let mut done = false;
            let mut total = 0;

            for i in band.clone().rev() {
                let bandbits = (FREQ_RANGE[i] as i32 * STATIC_ALLOC[center][i] as i32)
                    << (self.stereo_pkt as i32)
                    << self.lm >> 2;

                println!("bandbits {}", bandbits);

                let bandbits = if bandbits != 0 {
                    (bandbits + trim_offset[i]).max(0)
                } else {
                    bandbits
                } + boost[i];

                if bandbits >= threshold[i] || done {
                    done = true;
                    total += bandbits.min(caps[i]);
                } else {
                    if bandbits >= coded_channel_bits {
                        total += coded_channel_bits;
                    }
                }

                println!("total {} {}", total, available);

            }

            if total as usize > available {
                high = center - 1;
            } else {
                low = center + 1;
            }
            println!("{} {} {}", high, low, center);
        }

        println!("high {} low {}", high, low);

        high = low;
        low -= 1;

        let mut bits1 = [0; MAX_BANDS];
        let mut bits2 = [0; MAX_BANDS];

        println!("high {} low {}", high, low);

        for i in band.clone() {
            let bits_estimation = |idx: usize| -> i32 {
                let bits = (FREQ_RANGE[i] as i32 * STATIC_ALLOC[idx][i] as i32)
                    << (self.stereo_pkt as i32)
                    << self.lm >> 2;
                if bits != 0 {
                    (bits + trim_offset[i]).max(0)
                } else {
                    bits
                }
            };
            bits1[i] = bits_estimation(low);
            bits2[i] = bits_estimation(high);

            if boost[i] != 0 {
                if low != 0 {
                    bits1[i] += boost[i];
                }

                bits2[i] += boost[i];

                skip_startband = i;
            }

            bits2[i] = (bits2[i] - bits1[i]).max(0);
            println!("bits2 {}", bits2[i]);
        }

        const ALLOC_STEPS: usize = 6;

        low = 0;
        high = 1 << ALLOC_STEPS;

        for i in 0 .. ALLOC_STEPS {
            let center = (low + high) / 2;
            let mut done = false;
            let mut total = 0;

            for j in band.clone().rev() {
                let bits = bits1[j] + (center as i32 * bits2[j] >> ALLOC_STEPS);

                if bits >= threshold[j] || done {
                    done = true;
                    total += bits.min(caps[j]);
                } else if bits >= coded_channel_bits {
                    total += coded_channel_bits;
                }
            }

            if (total as usize > available) {
                high = center;
            } else {
                low = center;
            }
        }

        let mut done = false;
        let mut total = 0;

        for i in band.clone().rev() {
            let mut bits = bits1[i] + (low as i32 * bits2[i] >> ALLOC_STEPS);

            if bits >= threshold[i] || done {
                done = true;
            } else {
                bits = if bits >= coded_channel_bits {
                    coded_channel_bits
                } else {
                    0
                }
            }

            let bits = bits.min(caps[i]);
            self.pulses[i] = bits;
            total += bits;

            println!("total {}", total);
        }

    }

    pub fn decode(
        &mut self,
        rd: &mut RangeDecoder,
        out_buf: &mut [f32],
        frame_duration: FrameDuration,
        band: Range<usize>,
    ) {
        assert!(band.end <= MAX_BANDS);

        let frame_size = frame_duration as usize;

        self.lm = (frame_size / SHORT_BLOCKSIZE).ilog() - 1;

        let silence = if rd.available() > 0 {
            rd.decode_logp(15)
        } else {
            true
        };

        println!("silence {}", silence);

        if silence {
            // Pretend we are at the end of the buffer
            rd.to_end();
        }

        self.reset_gains();
        if band.start == 0 && rd.available() >= 16 {
            self.parse_postfilter(rd);
        }

        let transient = if self.lm != 0 && rd.available() >= 3 {
            rd.decode_logp(3)
        } else {
            false
        };

        println!("duration {}, transient {}", self.lm, transient);

        self.blocks = if transient { 1 << self.lm } else { 1 };
        self.blocksize = frame_size / self.blocks;

        if !self.stereo_pkt {
            let (f0, f1) = self.frames.split_at_mut(1);

            f0[0]
                .energy
                .iter_mut()
                .zip(f1[0].energy.iter())
                .for_each(|(e0, &e1)| *e0 = e0.max(e1));
        }

        self.frames
            .iter_mut()
            .for_each(|f| f.collapse_masks.iter_mut().for_each(|c| *c = 0));

        self.decode_coarse_energy(rd, band.clone());
        self.decode_tf_changes(rd, band.clone(), transient);
        self.decode_allocation(rd, band.clone());
    }
}

mod test {}