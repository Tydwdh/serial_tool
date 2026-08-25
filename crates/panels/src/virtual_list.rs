use std::collections::HashMap;
use std::ops::Range;

#[derive(Debug, Default, Clone)]
struct FenwickTree {
    tree: Vec<f32>,
}

impl FenwickTree {
    fn from_values(values: &[f32]) -> Self {
        let mut tree = Self {
            tree: vec![0.0; values.len() + 1],
        };
        for (index, &value) in values.iter().enumerate() {
            tree.add(index, value);
        }
        tree
    }

    fn len(&self) -> usize {
        self.tree.len().saturating_sub(1)
    }

    fn add(&mut self, index: usize, delta: f32) {
        let mut index = index + 1;
        while index < self.tree.len() {
            self.tree[index] += delta;
            index += index & index.wrapping_neg();
        }
    }

    fn append(&mut self, value: f32) {
        let index = self.tree.len();
        let low_bit = index & index.wrapping_neg();
        let start = index - low_bit;
        let existing_range = self.prefix_sum(index - 1) - self.prefix_sum(start);
        self.tree.push(existing_range + value);
    }

    fn prefix_sum(&self, count: usize) -> f32 {
        let mut index = count.min(self.len());
        let mut sum = 0.0;
        while index > 0 {
            sum += self.tree[index];
            index &= index - 1;
        }
        sum
    }

    fn total(&self) -> f32 {
        self.prefix_sum(self.len())
    }

    /// 返回包含给定内容偏移的行索引。
    ///
    /// 偏移恰好落在行边界时返回后一行；超出末尾时返回最后一行。
    fn index_at_offset(&self, offset: f32) -> usize {
        let len = self.len();
        if len == 0 {
            return 0;
        }

        let target = offset.max(0.0);
        if target >= self.total() {
            return len - 1;
        }

        let mut index = 0;
        let mut sum = 0.0;
        let mut step = len.next_power_of_two() >> 1;
        while step > 0 {
            let next = index + step;
            if next <= len && sum + self.tree[next] <= target {
                sum += self.tree[next];
                index = next;
            }
            step >>= 1;
        }
        index.min(len - 1)
    }
}

/// 终端虚拟列表的稳定行高索引。
///
/// 行高只在实际布局过的行上更新；未布局行使用默认高度。Fenwick 树让总高度、
/// 行顶部位置和视口范围查询都保持 O(log n)，不会因为历史记录增长而每帧遍历全部行。
#[derive(Debug, Default)]
pub(crate) struct VirtualRowIndex {
    ids: Vec<u64>,
    heights: Vec<f32>,
    fenwick: FenwickTree,
    layout_key: Option<u64>,
    default_height: f32,
}

impl VirtualRowIndex {
    pub(crate) fn clear(&mut self) {
        self.ids.clear();
        self.heights.clear();
        self.fenwick = FenwickTree::default();
        self.layout_key = None;
    }

    pub(crate) fn needs_sync(&self, layout_key: u64, default_height: f32) -> bool {
        self.layout_key != Some(layout_key)
            || (self.default_height - default_height.max(1.0)).abs() > f32::EPSILON
    }

    /// 追加当前 ID 列表尾部的新行。
    ///
    /// 调用方必须已经确认新列表只是在旧列表尾部追加；这样可以避免每帧再次比较
    /// 全部稳定 ID。若布局参数不一致或列表并非增长，则回退到完整同步。
    pub(crate) fn append_ids(&mut self, ids: &[u64], layout_key: u64, default_height: f32) -> bool {
        let default_height = default_height.max(1.0);
        if self.layout_key == Some(layout_key)
            && (self.default_height - default_height).abs() <= f32::EPSILON
            && ids.len() >= self.ids.len()
            && ids.len() > self.ids.len()
        {
            for &id in &ids[self.ids.len()..] {
                self.ids.push(id);
                self.heights.push(default_height);
                self.fenwick.append(default_height);
            }
            return true;
        }
        self.sync_ids(ids, layout_key, default_height)
    }

    /// 同步筛选后的稳定 ID。
    ///
    /// 连续接收时通常只是尾部追加，走 O(log n) 的 Fenwick append；筛选结果或布局
    /// 参数发生变化时才重建索引，并尽可能按 ID 保留已有实测行高。
    pub(crate) fn sync_ids(&mut self, ids: &[u64], layout_key: u64, default_height: f32) -> bool {
        let default_height = default_height.max(1.0);
        if self.layout_key == Some(layout_key) && self.ids == ids {
            if (self.default_height - default_height).abs() > f32::EPSILON {
                let old_default = self.default_height;
                self.default_height = default_height;
                for (index, height) in self.heights.iter_mut().enumerate() {
                    if (*height - old_default).abs() <= f32::EPSILON {
                        *height = default_height;
                        self.fenwick.add(index, default_height - old_default);
                    }
                }
                return true;
            }
            return false;
        }

        let layout_changed = self.layout_key != Some(layout_key);
        self.default_height = default_height;
        self.layout_key = Some(layout_key);

        if !layout_changed && ids.len() >= self.ids.len() && ids.starts_with(&self.ids) {
            for &id in &ids[self.ids.len()..] {
                self.ids.push(id);
                self.heights.push(default_height);
                self.fenwick.append(default_height);
            }
            return true;
        }

        let old_heights: HashMap<u64, f32> = if layout_changed {
            HashMap::new()
        } else {
            self.ids
                .iter()
                .copied()
                .zip(self.heights.iter().copied())
                .collect()
        };
        self.ids = ids.to_vec();
        self.heights = self
            .ids
            .iter()
            .map(|id| old_heights.get(id).copied().unwrap_or(default_height))
            .collect();
        self.fenwick = FenwickTree::from_values(&self.heights);
        true
    }

    /// 同步带有逐行估算高度的列表。
    ///
    /// Log 的换行高度并不是常量，因此它不能直接使用 `sync_ids` 的单一默认高度。
    /// 估算值只在 ID 列表或布局参数变化时读取；稳定帧不会重新构造整套行高数组。
    pub(crate) fn sync_rows(
        &mut self,
        ids: &[u64],
        estimated_heights: &[f32],
        layout_key: u64,
    ) -> bool {
        debug_assert_eq!(ids.len(), estimated_heights.len());
        if ids.len() != estimated_heights.len() {
            return false;
        }

        if self.layout_key == Some(layout_key) && self.ids == ids {
            return false;
        }

        let layout_changed = self.layout_key != Some(layout_key);
        let old_heights: HashMap<u64, f32> = if layout_changed {
            HashMap::new()
        } else {
            self.ids
                .iter()
                .copied()
                .zip(self.heights.iter().copied())
                .collect()
        };

        self.layout_key = Some(layout_key);
        self.default_height = estimated_heights.first().copied().unwrap_or(1.0).max(1.0);
        self.ids = ids.to_vec();
        self.heights = ids
            .iter()
            .zip(estimated_heights.iter().copied())
            .map(|(id, estimate)| old_heights.get(id).copied().unwrap_or(estimate.max(1.0)))
            .collect();
        self.fenwick = FenwickTree::from_values(&self.heights);
        true
    }

    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }

    pub(crate) fn matches_layout(&self, layout_key: u64, row_count: usize) -> bool {
        self.layout_key == Some(layout_key) && self.len() == row_count
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub(crate) fn total_height(&self) -> f32 {
        self.fenwick.total()
    }

    pub(crate) fn row_top(&self, index: usize) -> f32 {
        self.fenwick.prefix_sum(index.min(self.len()))
    }

    pub(crate) fn height(&self, index: usize) -> f32 {
        self.heights
            .get(index)
            .copied()
            .unwrap_or(self.default_height)
    }

    pub(crate) fn index_at_offset(&self, offset: f32) -> Option<usize> {
        (!self.is_empty()).then(|| self.fenwick.index_at_offset(offset))
    }

    pub(crate) fn set_height(&mut self, index: usize, height: f32) -> bool {
        let Some(current) = self.heights.get_mut(index) else {
            return false;
        };
        let height = height.max(1.0);
        let delta = height - *current;
        if delta.abs() <= f32::EPSILON {
            return false;
        }
        *current = height;
        self.fenwick.add(index, delta);
        true
    }

    pub(crate) fn visible_range(
        &self,
        scroll_offset: f32,
        viewport_height: f32,
        overscan: f32,
    ) -> Range<usize> {
        if self.is_empty() {
            return 0..0;
        }

        let start_offset = (scroll_offset - overscan).max(0.0);
        let end_offset =
            (scroll_offset + viewport_height.max(0.0) + overscan).min(self.total_height());
        let start = self.fenwick.index_at_offset(start_offset);
        let end = self.fenwick.index_at_offset(end_offset).saturating_add(1);
        start..end.min(self.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenwick_prefix_and_append_are_consistent() {
        let mut tree = FenwickTree::from_values(&[10.0, 20.0, 30.0]);
        assert_eq!(tree.prefix_sum(0), 0.0);
        assert_eq!(tree.prefix_sum(2), 30.0);
        assert_eq!(tree.total(), 60.0);

        tree.append(40.0);
        assert_eq!(tree.prefix_sum(3), 60.0);
        assert_eq!(tree.total(), 100.0);
        assert_eq!(tree.index_at_offset(0.0), 0);
        assert_eq!(tree.index_at_offset(10.0), 1);
        assert_eq!(tree.index_at_offset(59.9), 2);
        assert_eq!(tree.index_at_offset(60.0), 3);
    }

    #[test]
    fn visible_range_uses_pixel_overscan() {
        let mut index = VirtualRowIndex::default();
        index.sync_ids(&[1, 2, 3, 4, 5], 1, 10.0);
        assert_eq!(index.visible_range(20.0, 10.0, 10.0), 1..5);
        assert_eq!(index.visible_range(25.0, 5.0, 0.0), 2..4);
    }

    #[test]
    fn measured_height_updates_prefix_positions() {
        let mut index = VirtualRowIndex::default();
        index.sync_ids(&[1, 2, 3], 1, 10.0);
        assert!(index.set_height(1, 30.0));
        assert_eq!(index.row_top(2), 40.0);
        assert_eq!(index.total_height(), 50.0);
        assert!(!index.set_height(1, 30.0));
    }

    #[test]
    fn append_preserves_measured_heights() {
        let mut index = VirtualRowIndex::default();
        index.sync_ids(&[1, 2], 1, 10.0);
        index.set_height(0, 20.0);
        assert!(index.sync_ids(&[1, 2, 3], 1, 10.0));
        assert_eq!(index.height(0), 20.0);
        assert_eq!(index.total_height(), 40.0);
    }

    #[test]
    fn append_ids_updates_only_the_new_tail() {
        let mut index = VirtualRowIndex::default();
        index.sync_ids(&[1, 2], 1, 10.0);
        index.set_height(0, 20.0);
        assert!(index.append_ids(&[1, 2, 3, 4], 1, 10.0));
        assert_eq!(index.height(0), 20.0);
        assert_eq!(index.height(2), 10.0);
        assert_eq!(index.total_height(), 50.0);
    }

    #[test]
    fn layout_change_resets_heights_to_new_default() {
        let mut index = VirtualRowIndex::default();
        index.sync_ids(&[1, 2], 1, 10.0);
        index.set_height(0, 20.0);
        index.sync_ids(&[1, 2], 2, 12.0);
        assert_eq!(index.height(0), 12.0);
        assert_eq!(index.total_height(), 24.0);
    }

    #[test]
    #[ignore = "fixed pressure benchmark; run with --release --ignored --nocapture"]
    fn pressure_50k_rows_stays_indexed() {
        let ids: Vec<u64> = (0..50_000).map(|id| id + 1).collect();
        let estimates: Vec<f32> = ids
            .iter()
            .map(|id| if id % 17 == 0 { 32.0 } else { 16.0 })
            .collect();
        let started = std::time::Instant::now();
        let mut index = VirtualRowIndex::default();
        index.sync_rows(&ids, &estimates, 1);
        let mut visited = 0usize;
        for offset in (0..10_000).step_by(97) {
            visited += index.visible_range(offset as f32, 480.0, 320.0).len();
        }
        let elapsed = started.elapsed();
        println!(
            "virtual_rows pressure rows={} visited={} total_height={:.0} elapsed={:?}",
            index.len(),
            visited,
            index.total_height(),
            elapsed
        );
        assert_eq!(index.len(), 50_000);
        assert!(visited > 0);
    }
}
