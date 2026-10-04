//! CPython's `random.Random(seed)` for an integer seed: the MT19937 state
//! `random_seed` builds, `getrandbits`, `_randbelow_with_getrandbits`, and
//! `sample`, so a sample matches Python's draw for draw.

use std::collections::HashSet;

const N: usize = 624;
const M: usize = 397;

pub(crate) struct PyRandom {
    state: [u32; N],
    index: usize,
}

impl PyRandom {
    /// `random.Random(seed)` for a seed below 2**32.
    pub(crate) fn new(seed: u32) -> Self {
        let mut random = Self {
            state: [0; N],
            index: N,
        };
        random.init_by_array(&[seed]);
        random
    }

    fn init_genrand(&mut self, seed: u32) {
        self.state[0] = seed;
        for i in 1..N {
            let previous = self.state[i - 1];
            self.state[i] = 1_812_433_253_u32
                .wrapping_mul(previous ^ (previous >> 30))
                .wrapping_add(i as u32);
        }
        self.index = N;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        self.init_genrand(19_650_218);
        let (mut i, mut j) = (1_usize, 0_usize);
        for _ in 0..N.max(key.len()) {
            let previous = self.state[i - 1];
            self.state[i] = (self.state[i] ^ (previous ^ (previous >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                self.state[0] = self.state[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..N - 1 {
            let previous = self.state[i - 1];
            self.state[i] = (self.state[i]
                ^ (previous ^ (previous >> 30)).wrapping_mul(1_566_083_941))
            .wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                self.state[0] = self.state[N - 1];
                i = 1;
            }
        }
        self.state[0] = 0x8000_0000;
    }

    fn genrand_u32(&mut self) -> u32 {
        const UPPER: u32 = 0x8000_0000;
        const LOWER: u32 = 0x7fff_ffff;
        const MATRIX: u32 = 0x9908_b0df;
        if self.index >= N {
            for k in 0..N {
                let y = (self.state[k] & UPPER) | (self.state[(k + 1) % N] & LOWER);
                let mut next = self.state[(k + M) % N] ^ (y >> 1);
                if y & 1 != 0 {
                    next ^= MATRIX;
                }
                self.state[k] = next;
            }
            self.index = 0;
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// `getrandbits(k)` for `k <= 32`.
    fn getrandbits(&mut self, bits: u32) -> u64 {
        if bits == 0 {
            return 0;
        }
        u64::from(self.genrand_u32() >> (32 - bits))
    }

    /// `_randbelow_with_getrandbits(n)` for `0 < n < 2**32`.
    fn randbelow(&mut self, n: u64) -> u64 {
        let bits = 64 - n.leading_zeros();
        let mut r = self.getrandbits(bits);
        while r >= n {
            r = self.getrandbits(bits);
        }
        r
    }

    /// `sample(population, k)`: the chosen indices, in draw order.
    pub(crate) fn sample(&mut self, n: usize, k: usize) -> Vec<usize> {
        let mut setsize = 21_usize;
        if k > 5 {
            // `4 ** ceil(log(k * 3, 4))`.
            let exponent = ((k * 3) as f64).ln() / 4_f64.ln();
            setsize += 4_usize.pow(exponent.ceil() as u32);
        }
        let mut result = Vec::with_capacity(k);
        if n <= setsize {
            let mut pool: Vec<usize> = (0..n).collect();
            for i in 0..k {
                let j = self.randbelow((n - i) as u64) as usize;
                result.push(pool[j]);
                pool[j] = pool[n - i - 1];
            }
        } else {
            let mut selected = HashSet::new();
            for _ in 0..k {
                let mut j = self.randbelow(n as u64) as usize;
                while selected.contains(&j) {
                    j = self.randbelow(n as u64) as usize;
                }
                selected.insert(j);
                result.push(j);
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::PyRandom;

    #[test]
    fn matches_cpython_seed_zero() {
        // `random.Random(0).getrandbits(32)` three times.
        let mut random = PyRandom::new(0);
        let draws: Vec<u32> = (0..3).map(|_| random.genrand_u32()).collect();
        assert_eq!(draws, vec![3_626_764_237, 1_654_615_998, 3_255_389_356]);
        // `random.Random(0).sample(range(10000), 5)`.
        assert_eq!(
            PyRandom::new(0).sample(10_000, 5),
            vec![6311, 6890, 663, 4242, 8376]
        );
        // `random.Random(0).sample(range(20), 4)` takes the pool path.
        assert_eq!(PyRandom::new(0).sample(20, 4), vec![12, 13, 1, 8]);
        // `random.Random(0).sample(range(12000), 500)[-3:]`.
        assert_eq!(
            PyRandom::new(0).sample(12_000, 500)[497..],
            [11139, 6934, 8539]
        );
    }
}
