// requires nightly rust for portable_simd feature

#[cfg(feature = "simd")]
use std::simd::{u64x4, SimdPartialEq, ToBitMask};

use crate::performance::Slot;

/// batch check if any of the target slots match the given slots
/// uses simd for parallel comparison when available
#[cfg(feature = "simd")]
pub fn batch_contains_slot(slots: &[Slot], targets: &[Slot]) -> Vec<bool> {
    let mut results = Vec::with_capacity(targets.len());
    
    // process in chunks of 4
    for target_chunk in targets.chunks(4) {
        let mut chunk_results = [false; 4];
        let chunk_len = target_chunk.len();
        
        // create simd vector from chunk (pad with 0 if needed)
        let mut target_array = [0u64; 4];
        for (i, &target) in target_chunk.iter().enumerate() {
            target_array[i] = target;
        }
        let target_vec = u64x4::from_array(target_array);
        
        // check against all slots
        for &slot in slots {
            let slot_vec = u64x4::splat(slot);
            let matches = slot_vec.simd_eq(target_vec);
            let mask = matches.to_bitmask();
            
            for i in 0..chunk_len {
                if (mask >> i) & 1 == 1 {
                    chunk_results[i] = true;
                }
            }
        }
        
        // add results
        results.extend_from_slice(&chunk_results[..chunk_len]);
    }
    
    results
}

/// fallback
#[cfg(not(feature = "simd"))]
pub fn batch_contains_slot(slots: &[Slot], targets: &[Slot]) -> Vec<bool> {
    targets.iter()
        .map(|target| slots.contains(target))
        .collect()
}

/// batch calculate sum of u64 values
#[cfg(feature = "simd")]
pub fn simd_sum_u64(values: &[u64]) -> u64 {
    let mut sum = 0u64;
    let chunks = values.chunks_exact(4);
    let remainder = chunks.remainder();
    
    // process full chunks
    for chunk in chunks {
        let vec = u64x4::from_slice(chunk);
        sum += vec.reduce_sum();
    }
    
    // process remainder
    for &val in remainder {
        sum += val;
    }
    
    sum
}

/// fallback
#[cfg(not(feature = "simd"))]
pub fn simd_sum_u64(values: &[u64]) -> u64 {
    values.iter().sum()
}

/// batch find minimum latency
#[cfg(feature = "simd")]
pub fn simd_min_latency(latencies: &[u64]) -> Option<u64> {
    if latencies.is_empty() {
        return None;
    }
    
    let mut min = u64::MAX;
    let chunks = latencies.chunks_exact(4);
    let remainder = chunks.remainder();
    
    // process full chunks
    for chunk in chunks {
        let vec = u64x4::from_slice(chunk);
        min = min.min(vec.reduce_min());
    }
    
    // process remainder
    for &val in remainder {
        min = min.min(val);
    }
    
    Some(min)
}

/// fallback
#[cfg(not(feature = "simd"))]
pub fn simd_min_latency(latencies: &[u64]) -> Option<u64> {
    latencies.iter().cloned().min()
}