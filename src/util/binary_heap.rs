pub struct BinaryHeap<K, I> {
    heap_num: u32,
    heap_size: u32,
    index_size: u32,
    heap: Vec<I>,
    keys: Vec<K>,
    heap_indices: Vec<u32>,
}

impl<K, I> BinaryHeap<K, I>
where
    K: PartialOrd + Copy + Default,
    I: Into<u32> + From<u32> + Copy + PartialEq,
{
    pub fn new(heap_size: u32, index_size: u32) -> Self {
        Self {
            heap_num: 0,
            heap_size,
            index_size,
            heap: vec![0u32.into(); heap_size as usize],
            keys: vec![K::default(); index_size as usize],
            heap_indices: vec![0xffffffff; index_size as usize],
        }
    }

    pub fn clear(&mut self) {
        self.heap_num = 0;
        self.heap_indices.fill(0xffffffff);
    }

    pub fn resize(&mut self, new_heap_size: u32, new_index_size: u32) {
        if new_heap_size != self.heap_size {
            self.heap.resize(new_heap_size as usize, 0u32.into());
            self.heap_size = new_heap_size;
        }
        if new_index_size != self.index_size {
            self.keys.resize(new_index_size as usize, K::default());
            self.heap_indices.resize(new_index_size as usize, 0xffffffff);
            self.index_size = new_index_size;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.heap_num == 0
    }

    pub fn num(&self) -> u32 {
        self.heap_num
    }

    pub fn is_present(&self, index: I) -> bool {
        let idx: u32 = index.into();
        if idx >= self.index_size {
            return false;
        }
        self.heap_indices[idx as usize] != 0xffffffff
    }

    pub fn get_key(&self, index: I) -> K {
        let idx: u32 = index.into();
        self.keys[idx as usize]
    }

    pub fn top(&self) -> I {
        assert!(self.heap_num > 0);
        self.heap[0]
    }

    pub fn pop(&mut self) -> I {
        assert!(self.heap_num > 0);
        let index = self.heap[0];
        self.heap_num -= 1;
        if self.heap_num > 0 {
            self.heap[0] = self.heap[self.heap_num as usize];
            let top_idx: u32 = self.heap[0].into();
            self.heap_indices[top_idx as usize] = 0;
            self.down_heap(0);
        }
        let popped_idx: u32 = index.into();
        self.heap_indices[popped_idx as usize] = 0xffffffff;
        index
    }

    pub fn add(&mut self, key: K, index: I) {
        if self.heap_num == self.heap_size {
            let new_size = 32.max(self.heap_size * 2);
            self.heap.resize(new_size as usize, 0u32.into());
            self.heap_size = new_size;
        }

        let idx: u32 = index.into();
        if idx >= self.index_size {
            let new_size = 32.max((idx + 1).next_power_of_two());
            self.keys.resize(new_size as usize, K::default());
            self.heap_indices.resize(new_size as usize, 0xffffffff);
            self.index_size = new_size;
        }

        let heap_index = self.heap_num;
        self.heap_num += 1;
        self.heap[heap_index as usize] = index;
        self.keys[idx as usize] = key;
        self.heap_indices[idx as usize] = heap_index;

        self.up_heap(heap_index);
    }

    pub fn update(&mut self, key: K, index: I) {
        let idx: u32 = index.into();
        self.keys[idx as usize] = key;
        let heap_index = self.heap_indices[idx as usize];
        if heap_index > 0 {
            let parent = (heap_index - 1) >> 1;
            let parent_idx: u32 = self.heap[parent as usize].into();
            if key < self.keys[parent_idx as usize] {
                self.up_heap(heap_index);
                return;
            }
        }
        self.down_heap(heap_index);
    }

    pub fn remove(&mut self, index: I) {
        if !self.is_present(index) {
            return;
        }

        let idx: u32 = index.into();
        let old_key = self.keys[idx as usize];
        let heap_index = self.heap_indices[idx as usize];

        self.heap_num -= 1;
        if heap_index < self.heap_num {
            self.heap[heap_index as usize] = self.heap[self.heap_num as usize];
            let moved_idx: u32 = self.heap[heap_index as usize].into();
            self.heap_indices[moved_idx as usize] = heap_index;
            self.heap_indices[idx as usize] = 0xffffffff;

            let new_key = self.keys[moved_idx as usize];
            if new_key < old_key {
                self.up_heap(heap_index);
            } else {
                self.down_heap(heap_index);
            }
        } else {
            self.heap_indices[idx as usize] = 0xffffffff;
        }
    }

    fn up_heap(&mut self, mut heap_index: u32) {
        let moving = self.heap[heap_index as usize];
        let moving_idx: u32 = moving.into();
        let moving_key = self.keys[moving_idx as usize];

        while heap_index > 0 {
            let parent = (heap_index - 1) >> 1;
            let parent_idx: u32 = self.heap[parent as usize].into();
            if moving_key < self.keys[parent_idx as usize] {
                self.heap[heap_index as usize] = self.heap[parent as usize];
                let h_idx: u32 = self.heap[heap_index as usize].into();
                self.heap_indices[h_idx as usize] = heap_index;
                heap_index = parent;
            } else {
                break;
            }
        }

        self.heap[heap_index as usize] = moving;
        self.heap_indices[moving_idx as usize] = heap_index;
    }

    fn down_heap(&mut self, mut heap_index: u32) {
        let moving = self.heap[heap_index as usize];
        let moving_idx: u32 = moving.into();
        let moving_key = self.keys[moving_idx as usize];

        loop {
            let mut smallest = heap_index;
            let left = (heap_index << 1) + 1;
            let right = left + 1;

            if left < self.heap_num {
                let left_idx: u32 = self.heap[left as usize].into();
                if self.keys[left_idx as usize] < moving_key {
                    smallest = left;
                }
            }

            if right < self.heap_num {
                let right_idx: u32 = self.heap[right as usize].into();
                let smallest_idx: u32 = self.heap[smallest as usize].into();
                if self.keys[right_idx as usize] < self.keys[smallest_idx as usize] {
                    smallest = right;
                }
            }

            if smallest != heap_index {
                self.heap[heap_index as usize] = self.heap[smallest as usize];
                let h_idx: u32 = self.heap[heap_index as usize].into();
                self.heap_indices[h_idx as usize] = heap_index;
                heap_index = smallest;
            } else {
                break;
            }
        }

        self.heap[heap_index as usize] = moving;
        self.heap_indices[moving_idx as usize] = heap_index;
    }
}
