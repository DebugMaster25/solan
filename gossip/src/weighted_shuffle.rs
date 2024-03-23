//! The `weighted_shuffle` module provides an iterator over shuffled weights.

use {
    num_traits::CheckedAdd,
    rand::{
        distributions::uniform::{SampleUniform, UniformSampler},
        Rng,
    },
    std::ops::{AddAssign, Sub, SubAssign},
};

/// Implements an iterator where indices are shuffled according to their
/// weights:
///   - Returned indices are unique in the range [0, weights.len()).
///   - Higher weighted indices tend to appear earlier proportional to their
///     weight.
///   - Zero weighted indices are shuffled and appear only at the end, after
///     non-zero weighted indices.
#[derive(Clone)]
pub struct WeightedShuffle<T> {
    // Underlying array implementing binary tree.
    // tree[i] is the sum of weights in the left sub-tree of node i.
    tree: Vec<T>,
    // Current sum of all weights, excluding already sampled ones.
    weight: T,
    zeros: Vec<usize>, // Indices of zero weighted entries.
}

impl<T> WeightedShuffle<T>
where
    T: Copy + Default + PartialOrd + AddAssign + CheckedAdd,
{
    /// If weights are negative or overflow the total sum
    /// they are treated as zero.
    pub fn new(name: &'static str, weights: &[T]) -> Self {
        let zero = <T as Default>::default();
        let mut tree = vec![zero; get_tree_size(weights.len())];
        let mut sum = zero;
        let mut zeros = Vec::default();
        let mut num_negative = 0;
        let mut num_overflow = 0;
        for (k, &weight) in weights.iter().enumerate() {
            #[allow(clippy::neg_cmp_op_on_partial_ord)]
            // weight < zero does not work for NaNs.
            if !(weight >= zero) {
                zeros.push(k);
                num_negative += 1;
                continue;
            }
            if weight == zero {
                zeros.push(k);
                continue;
            }
            sum = match sum.checked_add(&weight) {
                Some(val) => val,
                None => {
                    zeros.push(k);
                    num_overflow += 1;
                    continue;
                }
            };
            let mut index = tree.len() + k;
            while index != 0 {
                let offset = index & 1;
                index = (index - 1) >> 1;
                if offset > 0 {
                    tree[index] += weight;
                }
            }
        }
        if num_negative > 0 {
            datapoint_error!("weighted-shuffle-negative", (name, num_negative, i64));
        }
        if num_overflow > 0 {
            datapoint_error!("weighted-shuffle-overflow", (name, num_overflow, i64));
        }
        Self {
            tree,
            weight: sum,
            zeros,
        }
    }
}

impl<T> WeightedShuffle<T>
where
    T: Copy + Default + PartialOrd + AddAssign + SubAssign + Sub<Output = T>,
{
    // Removes given weight at index k.
    fn remove(&mut self, k: usize, weight: T) {
        self.weight -= weight;
        let mut index = self.tree.len() + k;
        while index != 0 {
            let offset = index & 1;
            index = (index - 1) >> 1;
            if offset > 0 {
                self.tree[index] -= weight;
            }
        }
    }

    // Returns smallest index such that cumsum of weights[..=k] > val,
    // along with its respective weight.
    fn search(&self, mut val: T) -> (/*index:*/ usize, /*weight:*/ T) {
        let zero = <T as Default>::default();
        debug_assert!(val >= zero);
        debug_assert!(val < self.weight);
        let mut index = 0;
        let mut weight = self.weight;
        while index < self.tree.len() {
            if val < self.tree[index] {
                weight = self.tree[index];
                index = (index << 1) + 1;
            } else {
                weight -= self.tree[index];
                val -= self.tree[index];
                index = (index << 1) + 2;
            }
        }
        (index - self.tree.len(), weight)
    }

    pub fn remove_index(&mut self, k: usize) {
        let mut index = self.tree.len() + k;
        let mut weight = <T as Default>::default(); // zero
        while index != 0 {
            let offset = index & 1;
            index = (index - 1) >> 1;
            if offset > 0 {
                if self.tree[index] != weight {
                    self.remove(k, self.tree[index] - weight);
                } else {
                    self.remove_zero(k);
                }
                return;
            }
            weight += self.tree[index];
        }
        if self.weight != weight {
            self.remove(k, self.weight - weight);
        } else {
            self.remove_zero(k);
        }
    }

    fn remove_zero(&mut self, k: usize) {
        if let Some(index) = self.zeros.iter().position(|&ix| ix == k) {
            self.zeros.remove(index);
        }
    }
}

impl<T> WeightedShuffle<T>
where
    T: Copy + Default + PartialOrd + AddAssign + SampleUniform + SubAssign + Sub<Output = T>,
{
    // Equivalent to weighted_shuffle.shuffle(&mut rng).next()
    pub fn first<R: Rng>(&self, rng: &mut R) -> Option<usize> {
        let zero = <T as Default>::default();
        if self.weight > zero {
            let sample = <T as SampleUniform>::Sampler::sample_single(zero, self.weight, rng);
            let (index, _weight) = WeightedShuffle::search(self, sample);
            return Some(index);
        }
        if self.zeros.is_empty() {
            return None;
        }
        let index = <usize as SampleUniform>::Sampler::sample_single(0usize, self.zeros.len(), rng);
        self.zeros.get(index).copied()
    }
}

impl<'a, T: 'a> WeightedShuffle<T>
where
    T: Copy + Default + PartialOrd + AddAssign + SampleUniform + SubAssign + Sub<Output = T>,
{
    pub fn shuffle<R: Rng>(mut self, rng: &'a mut R) -> impl Iterator<Item = usize> + 'a {
        std::iter::from_fn(move || {
            let zero = <T as Default>::default();
            if self.weight > zero {
                let sample = <T as SampleUniform>::Sampler::sample_single(zero, self.weight, rng);
                let (index, weight) = WeightedShuffle::search(&self, sample);
                self.remove(index, weight);
                return Some(index);
            }
            if self.zeros.is_empty() {
                return None;
            }
            let index =
                <usize as SampleUniform>::Sampler::sample_single(0usize, self.zeros.len(), rng);
            Some(self.zeros.swap_remove(index))
        })
    }
}

// Maps number of items to the "internal" size of the binary tree "implicitly"
// holding those items on the leaves.
fn get_tree_size(count: usize) -> usize {
    let shift = usize::BITS
        - count.leading_zeros()
        - if count.is_power_of_two() && count != 1 {
            1
        } else {
            0
        };
    (1usize << shift) - 1
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        rand::SeedableRng,
        rand_chacha::ChaChaRng,
        std::{convert::TryInto, iter::repeat_with},
    };

    fn weighted_shuffle_slow<R>(rng: &mut R, mut weights: Vec<u64>) -> Vec<usize>
    where
        R: Rng,
    {
        let mut shuffle = Vec::with_capacity(weights.len());
        let mut high: u64 = weights.iter().sum();
        let mut zeros: Vec<_> = weights
            .iter()
            .enumerate()
            .filter(|(_, w)| **w == 0)
            .map(|(i, _)| i)
            .collect();
        while high != 0 {
            let sample = rng.gen_range(0..high);
            let index = weights
                .iter()
                .scan(0, |acc, &w| {
                    *acc += w;
                    Some(*acc)
                })
                .position(|acc| sample < acc)
                .unwrap();
            shuffle.push(index);
            high -= weights[index];
            weights[index] = 0;
        }
        while !zeros.is_empty() {
            let index = <usize as SampleUniform>::Sampler::sample_single(0usize, zeros.len(), rng);
            shuffle.push(zeros.swap_remove(index));
        }
        shuffle
    }

    #[test]
    fn test_get_tree_size() {
        assert_eq!(get_tree_size(0), 0);
        assert_eq!(get_tree_size(1), 1);
        assert_eq!(get_tree_size(2), 1);
        assert_eq!(get_tree_size(3), 3);
        assert_eq!(get_tree_size(4), 3);
        for count in 5..9 {
            assert_eq!(get_tree_size(count), 7);
        }
        for count in 9..17 {
            assert_eq!(get_tree_size(count), 15);
        }
        for count in 17..33 {
            assert_eq!(get_tree_size(count), 31);
        }
        assert_eq!(get_tree_size((1 << 16) - 1), (1 << 16) - 1);
        assert_eq!(get_tree_size(1 << 16), (1 << 16) - 1);
        assert_eq!(get_tree_size((1 << 16) + 1), (1 << 17) - 1);
        assert_eq!(get_tree_size((1 << 17) - 1), (1 << 17) - 1);
        assert_eq!(get_tree_size(1 << 17), (1 << 17) - 1);
        assert_eq!(get_tree_size((1 << 17) + 1), (1 << 18) - 1);
    }

    // Asserts that empty weights will return empty shuffle.
    #[test]
    fn test_weighted_shuffle_empty_weights() {
        let weights = Vec::<u64>::new();
        let mut rng = rand::thread_rng();
        let shuffle = WeightedShuffle::new("", &weights);
        assert!(shuffle.clone().shuffle(&mut rng).next().is_none());
        assert!(shuffle.first(&mut rng).is_none());
    }

    // Asserts that zero weights will be shuffled.
    #[test]
    fn test_weighted_shuffle_zero_weights() {
        let weights = vec![0u64; 5];
        let seed = [37u8; 32];
        let mut rng = ChaChaRng::from_seed(seed);
        let shuffle = WeightedShuffle::new("", &weights);
        assert_eq!(
            shuffle.clone().shuffle(&mut rng).collect::<Vec<_>>(),
            [1, 4, 2, 3, 0]
        );
        let mut rng = ChaChaRng::from_seed(seed);
        assert_eq!(shuffle.first(&mut rng), Some(1));
    }

    // Asserts that each index is selected proportional to its weight.
    #[test]
    fn test_weighted_shuffle_sanity() {
        let seed: Vec<_> = (1..).step_by(3).take(32).collect();
        let seed: [u8; 32] = seed.try_into().unwrap();
        let mut rng = ChaChaRng::from_seed(seed);
        let weights = [1, 0, 1000, 0, 0, 10, 100, 0];
        let mut counts = [0; 8];
        for _ in 0..100000 {
            let mut shuffle = WeightedShuffle::new("", &weights).shuffle(&mut rng);
            counts[shuffle.next().unwrap()] += 1;
            let _ = shuffle.count(); // consume the rest.
        }
        assert_eq!(counts, [95, 0, 90069, 0, 0, 908, 8928, 0]);
        let mut counts = [0; 8];
        for _ in 0..100000 {
            let mut shuffle = WeightedShuffle::new("", &weights);
            shuffle.remove_index(5);
            shuffle.remove_index(3);
            shuffle.remove_index(1);
            let mut shuffle = shuffle.shuffle(&mut rng);
            counts[shuffle.next().unwrap()] += 1;
            let _ = shuffle.count(); // consume the rest.
        }
        assert_eq!(counts, [97, 0, 90862, 0, 0, 0, 9041, 0]);
    }

    #[test]
    fn test_weighted_shuffle_negative_overflow() {
        const SEED: [u8; 32] = [48u8; 32];
        let weights = [19i64, 23, 7, 0, 0, 23, 3, 0, 5, 0, 19, 29];
        let mut rng = ChaChaRng::from_seed(SEED);
        let shuffle = WeightedShuffle::new("", &weights);
        assert_eq!(
            shuffle.shuffle(&mut rng).collect::<Vec<_>>(),
            [8, 1, 5, 10, 11, 0, 2, 6, 9, 4, 3, 7]
        );
        // Negative weights and overflowing ones are treated as zero.
        let weights = [19, 23, 7, -57, i64::MAX, 23, 3, i64::MAX, 5, -79, 19, 29];
        let mut rng = ChaChaRng::from_seed(SEED);
        let shuffle = WeightedShuffle::new("", &weights);
        assert_eq!(
            shuffle.shuffle(&mut rng).collect::<Vec<_>>(),
            [8, 1, 5, 10, 11, 0, 2, 6, 9, 4, 3, 7]
        );
    }

    #[test]
    fn test_weighted_shuffle_hard_coded() {
        let weights = [
            78, 70, 38, 27, 21, 0, 82, 42, 21, 77, 77, 0, 17, 4, 50, 96, 0, 83, 33, 16, 72,
        ];
        let seed = [48u8; 32];
        let mut rng = ChaChaRng::from_seed(seed);
        let mut shuffle = WeightedShuffle::new("", &weights);
        assert_eq!(
            shuffle.clone().shuffle(&mut rng).collect::<Vec<_>>(),
            [2, 12, 18, 0, 14, 15, 17, 10, 1, 9, 7, 6, 13, 20, 4, 19, 3, 8, 11, 16, 5]
        );
        let mut rng = ChaChaRng::from_seed(seed);
        assert_eq!(shuffle.first(&mut rng), Some(2));
        let mut rng = ChaChaRng::from_seed(seed);
        shuffle.remove_index(11);
        shuffle.remove_index(3);
        shuffle.remove_index(15);
        shuffle.remove_index(0);
        assert_eq!(
            shuffle.clone().shuffle(&mut rng).collect::<Vec<_>>(),
            [4, 6, 1, 12, 19, 14, 17, 20, 2, 9, 10, 8, 7, 18, 13, 5, 16]
        );
        let mut rng = ChaChaRng::from_seed(seed);
        assert_eq!(shuffle.first(&mut rng), Some(4));
        let seed = [37u8; 32];
        let mut rng = ChaChaRng::from_seed(seed);
        let mut shuffle = WeightedShuffle::new("", &weights);
        assert_eq!(
            shuffle.clone().shuffle(&mut rng).collect::<Vec<_>>(),
            [19, 3, 15, 14, 6, 10, 17, 18, 9, 2, 4, 1, 0, 7, 8, 20, 12, 13, 16, 5, 11]
        );
        let mut rng = ChaChaRng::from_seed(seed);
        assert_eq!(shuffle.first(&mut rng), Some(19));
        shuffle.remove_index(16);
        shuffle.remove_index(8);
        shuffle.remove_index(20);
        shuffle.remove_index(5);
        shuffle.remove_index(19);
        shuffle.remove_index(4);
        let mut rng = ChaChaRng::from_seed(seed);
        assert_eq!(
            shuffle.clone().shuffle(&mut rng).collect::<Vec<_>>(),
            [17, 2, 9, 14, 6, 10, 12, 1, 15, 13, 7, 0, 18, 3, 11]
        );
        let mut rng = ChaChaRng::from_seed(seed);
        assert_eq!(shuffle.first(&mut rng), Some(17));
    }

    #[test]
    fn test_weighted_shuffle_match_slow() {
        let mut rng = rand::thread_rng();
        let weights: Vec<u64> = repeat_with(|| rng.gen_range(0..1000)).take(997).collect();
        for _ in 0..10 {
            let mut seed = [0u8; 32];
            rng.fill(&mut seed[..]);
            let mut rng = ChaChaRng::from_seed(seed);
            let shuffle = WeightedShuffle::new("", &weights);
            let shuffle: Vec<_> = shuffle.shuffle(&mut rng).collect();
            let mut rng = ChaChaRng::from_seed(seed);
            let shuffle_slow = weighted_shuffle_slow(&mut rng, weights.clone());
            assert_eq!(shuffle, shuffle_slow);
            let mut rng = ChaChaRng::from_seed(seed);
            let shuffle = WeightedShuffle::new("", &weights);
            assert_eq!(shuffle.first(&mut rng), Some(shuffle_slow[0]));
        }
    }

    #[test]
    fn test_weighted_shuffle_paranoid() {
        let mut rng = rand::thread_rng();
        for size in 0..1351 {
            let weights: Vec<_> = repeat_with(|| rng.gen_range(0..1000)).take(size).collect();
            let seed = rng.gen::<[u8; 32]>();
            let mut rng = ChaChaRng::from_seed(seed);
            let shuffle_slow = weighted_shuffle_slow(&mut rng.clone(), weights.clone());
            let shuffle = WeightedShuffle::new("", &weights);
            if size > 0 {
                assert_eq!(shuffle.first(&mut rng.clone()), Some(shuffle_slow[0]));
            }
            assert_eq!(shuffle.shuffle(&mut rng).collect::<Vec<_>>(), shuffle_slow);
        }
    }
}
