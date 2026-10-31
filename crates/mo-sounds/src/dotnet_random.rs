//! .NET's `new Random(seed)`, number for number, so what the C# app shuffled with a seed comes out in the same order
//! here (the type classifier's balancing and training order, the learned tags' split). It's Knuth's subtractive
//! generator, which .NET keeps for any `Random` made with a seed ("Net5CompatSeedImpl" in .NET's own code).
//!
//! Not for anything new: [`DotnetRandom`] is here only so Rust can follow what C# did step for step.

const MAX: i32 = i32::MAX;
const SEED: i32 = 161_803_398;

pub(crate) struct DotnetRandom {
    seeds: [i32; 56],
    next: usize,
    next_p: usize,
}

impl DotnetRandom {
    /// As `new Random(seed)`.
    pub(crate) fn new(seed: i32) -> DotnetRandom {
        let mut seeds = [0i32; 56];
        let subtraction = if seed == i32::MIN { MAX } else { seed.abs() };
        let mut mj = SEED.wrapping_sub(subtraction);
        seeds[55] = mj;
        let mut mk = 1i32;
        let mut ii = 0usize;
        for _ in 1..55 {
            ii += 21;
            if ii >= 55 {
                ii -= 55;
            }
            seeds[ii] = mk;
            mk = mj.wrapping_sub(mk);
            if mk < 0 {
                mk = mk.wrapping_add(MAX);
            }
            mj = seeds[ii];
        }
        for _ in 1..5 {
            for i in 1..56 {
                let mut n = i + 30;
                if n >= 55 {
                    n -= 55;
                }
                seeds[i] = seeds[i].wrapping_sub(seeds[1 + n]);
                if seeds[i] < 0 {
                    seeds[i] = seeds[i].wrapping_add(MAX);
                }
            }
        }
        DotnetRandom { seeds, next: 0, next_p: 21 }
    }

    /// As `Next()`: 0 up to (not including) `i32::MAX`.
    pub(crate) fn next(&mut self) -> i32 {
        let mut a = self.next + 1;
        if a >= 56 {
            a = 1;
        }
        let mut b = self.next_p + 1;
        if b >= 56 {
            b = 1;
        }
        let mut v = self.seeds[a].wrapping_sub(self.seeds[b]);
        if v == MAX {
            v -= 1;
        }
        if v < 0 {
            v = v.wrapping_add(MAX);
        }
        self.seeds[a] = v;
        self.next = a;
        self.next_p = b;
        v
    }

    /// As `Sample()`: 0 up to (not including) 1.
    fn sample(&mut self) -> f64 {
        f64::from(self.next()) * (1.0 / f64::from(MAX))
    }

    /// As `Next(min, max)` for a range that fits in an `i32` (all the app uses).
    pub(crate) fn next_in(&mut self, min: i32, max: i32) -> i32 {
        let range = i64::from(max) - i64::from(min);
        debug_assert!((0..=i64::from(MAX)).contains(&range));
        (self.sample() * range as f64) as i32 + min
    }

    /// As `Shuffle(array)`.
    pub(crate) fn shuffle<T>(&mut self, values: &mut [T]) {
        let n = values.len();
        for i in 0..n.saturating_sub(1) {
            let j = self.next_in(i as i32, n as i32) as usize;
            values.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_numbers_and_they_stay_in_range() {
        let mut a = DotnetRandom::new(7);
        let mut b = DotnetRandom::new(7);
        let mut c = DotnetRandom::new(8);
        let xs: Vec<i32> = (0..1000).map(|_| a.next()).collect();
        assert_eq!(xs, (0..1000).map(|_| b.next()).collect::<Vec<_>>());
        assert_ne!(xs, (0..1000).map(|_| c.next()).collect::<Vec<_>>());
        assert!(xs.iter().all(|v| (0..MAX).contains(v)));
        for _ in 0..1000 {
            let v = a.next_in(3, 10);
            assert!((3..10).contains(&v));
        }
    }

    #[test]
    fn it_gives_the_numbers_dotnet_gives() {
        // `new Random(0).Next()`, three times, in .NET.
        let mut r = DotnetRandom::new(0);
        assert_eq!([r.next(), r.next(), r.next()], [1_559_595_546, 1_755_192_844, 1_649_316_166]);
    }

    #[test]
    fn a_shuffle_keeps_every_value() {
        let mut v: Vec<usize> = (0..100).collect();
        DotnetRandom::new(7).shuffle(&mut v);
        assert_ne!(v, (0..100).collect::<Vec<_>>());
        v.sort_unstable();
        assert_eq!(v, (0..100).collect::<Vec<_>>());
        let mut one = [5];
        DotnetRandom::new(1).shuffle(&mut one);
        assert_eq!(one, [5]);
    }

    #[test]
    fn negative_and_smallest_seeds_work() {
        assert_eq!(DotnetRandom::new(-7).next(), DotnetRandom::new(7).next());
        let _ = DotnetRandom::new(i32::MIN).next();
    }
}
