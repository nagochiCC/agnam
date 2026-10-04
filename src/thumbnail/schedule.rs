use std::cmp::Reverse;
use std::collections::BinaryHeap;

pub(super) const BACKGROUND_BURST_SIZE: usize = 6;

fn distributed_anchors(page_count: usize) -> Vec<usize> {
    if page_count == 0 {
        return Vec::new();
    }

    let mut anchors = Vec::with_capacity(page_count);
    anchors.push(0);
    if page_count == 1 {
        return anchors;
    }
    anchors.push(page_count - 1);

    // (unselected page count, lower start first, left, right)
    let mut gaps = BinaryHeap::from([(page_count - 2, Reverse(0), 0, page_count - 1)]);
    while let Some((missing, _, left, right)) = gaps.pop() {
        if missing == 0 {
            continue;
        }
        let midpoint = left + (right - left) / 2;
        anchors.push(midpoint);

        let left_missing = midpoint.saturating_sub(left + 1);
        if left_missing > 0 {
            gaps.push((left_missing, Reverse(left), left, midpoint));
        }
        let right_missing = right.saturating_sub(midpoint + 1);
        if right_missing > 0 {
            gaps.push((right_missing, Reverse(midpoint), midpoint, right));
        }
    }
    anchors
}

pub(super) fn distributed_page_order(page_count: usize) -> Vec<usize> {
    let mut seen = vec![false; page_count];
    let mut order = Vec::with_capacity(page_count);
    for anchor in distributed_anchors(page_count) {
        for page_index in anchor_block(anchor, page_count).into_iter().flatten() {
            if !seen[page_index] {
                seen[page_index] = true;
                order.push(page_index);
            }
        }
    }
    order
}

fn anchor_block(anchor: usize, page_count: usize) -> [Option<usize>; 3] {
    [
        (anchor < page_count).then_some(anchor),
        anchor.checked_sub(1).filter(|&index| index < page_count),
        anchor.checked_add(1).filter(|&index| index < page_count),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn distributed_anchors_cover_edges_and_center_early() {
        let anchors = distributed_anchors(200);

        assert_eq!(&anchors[..2], &[0, 199]);
        assert!(anchors[2].abs_diff(99) <= 1);
        assert!(anchors[..7].iter().any(|&index| index.abs_diff(49) <= 1));
        assert!(anchors[..7].iter().any(|&index| index.abs_diff(149) <= 1));
    }

    #[test]
    fn each_anchor_expands_center_previous_next_without_duplicates() {
        assert_eq!(anchor_block(125, 200), [Some(125), Some(124), Some(126)]);
        assert_eq!(anchor_block(0, 200), [Some(0), None, Some(1)]);
        assert_eq!(anchor_block(199, 200), [Some(199), Some(198), None]);
        assert_eq!(distributed_page_order(1), vec![0]);
        assert_eq!(distributed_page_order(2), vec![0, 1]);
        assert_eq!(distributed_page_order(5), vec![0, 1, 4, 3, 2]);
    }

    #[test]
    fn distributed_order_covers_every_page_exactly_once() {
        for page_count in 0..=200 {
            let order = distributed_page_order(page_count);
            let unique: HashSet<_> = order.iter().copied().collect();

            assert_eq!(order.len(), page_count, "page_count={page_count}");
            assert_eq!(unique.len(), page_count, "page_count={page_count}");
            assert!(order.iter().all(|&index| index < page_count));
            assert!((0..page_count).all(|index| unique.contains(&index)));
        }
    }
}
