use sha2::{Digest, Sha256};

/// Computes the SGLang-compatible recursive SHA256 page hashes for a given sequence of token IDs.
///
/// Each page of size `page_size` is hashed recursively:
/// - Page 0 digest: `SHA256(tokens[0..page_size] as LE u32)`
/// - Page i digest: `SHA256(digest[i-1] ++ tokens[i*page_size..(i+1)*page_size] as LE u32)`
///
/// Returns a vector of page hashes (signed 64-bit integer taken from first 8 bytes in big-endian/LE).
pub fn compute_sglang_page_hashes(token_ids: &[u32], page_size: usize) -> Vec<i64> {
    if token_ids.is_empty() || page_size == 0 {
        return Vec::new();
    }

    let num_pages = token_ids.len() / page_size;
    let mut page_hashes = Vec::with_capacity(num_pages);
    let mut prev_digest: Option<[u8; 32]> = None;

    for i in 0..num_pages {
        let chunk = &token_ids[i * page_size..(i + 1) * page_size];
        let mut hasher = Sha256::new();

        if let Some(prev) = prev_digest {
            hasher.update(prev);
        }

        for &tok in chunk {
            hasher.update(tok.to_le_bytes());
        }

        let digest: [u8; 32] = hasher.finalize().into();
        // First 8 bytes converted to signed i64 in big-endian (SGLang standard radix page hash representation)
        let hash_i64 = i64::from_be_bytes(digest[0..8].try_into().expect("slice with exact length"));
        page_hashes.push(hash_i64);
        prev_digest = Some(digest);
    }

    page_hashes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sglang_page_hashes_empty() {
        assert!(compute_sglang_page_hashes(&[], 16).is_empty());
        assert!(compute_sglang_page_hashes(&[1, 2, 3], 0).is_empty());
    }

    #[test]
    fn test_sglang_page_hashes_deterministic() {
        let tokens: Vec<u32> = (0..64).collect();
        let hashes1 = compute_sglang_page_hashes(&tokens, 16);
        let hashes2 = compute_sglang_page_hashes(&tokens, 16);

        assert_eq!(hashes1.len(), 4);
        assert_eq!(hashes1, hashes2);
        // Ensure cascading hash dependency
        assert_ne!(hashes1[0], hashes1[1]);
    }

    #[test]
    fn test_sglang_golden_vectors() {
        // Golden vector from SGLang: chain([1,2,3,4], 4) -> [-3488128144981237669]
        let got = compute_sglang_page_hashes(&[1, 2, 3, 4], 4);
        assert_eq!(got, vec![-3488128144981237669_i64]);

        // Golden vector from SGLang: chain([10,20,30,40,50,60,70,80], 2)
        let got2 = compute_sglang_page_hashes(&[10, 20, 30, 40, 50, 60, 70, 80], 2);
        assert_eq!(
            got2,
            vec![
                978178666101069530_i64,
                -895308556211281782_i64,
                -8033692805846017938_i64,
                835415944263129316_i64,
            ]
        );
    }
}
