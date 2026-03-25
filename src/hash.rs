pub fn murmur_finalize32(mut hash: u32) -> u32 {
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x85ebca6b);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(0xc2b2ae35);
    hash ^= hash >> 16;
    hash
}

pub fn murmur32(data: &[u32]) -> u32 {
    let mut hash: u32 = 0;
    for &element in data {
        let mut e = element.wrapping_mul(0xcc9e2d51);
        e = (e << 15) | (e >> (32 - 15));
        e = e.wrapping_mul(0x1b873593);

        hash ^= e;
        hash = (hash << 13) | (hash >> (32 - 13));
        hash = hash.wrapping_mul(5).wrapping_add(0xe6546b64);
    }
    murmur_finalize32(hash)
}

pub struct FHashTable {
    hash_size: u32,
    hash_mask: u32,
    index_size: u32,
    hash: Vec<u32>,
    next_index: Vec<u32>,
}

impl FHashTable {
    pub fn new(hash_size: u32, index_size: u32) -> Self {
        assert!(hash_size > 0);
        assert!(hash_size.is_power_of_two());

        let h = Self {
            hash_size,
            hash_mask: hash_size - 1,
            index_size,
            hash: vec![0xffffffff; hash_size as usize],
            next_index: vec![0xffffffff; index_size as usize],
        };
        h
    }

    pub fn clear(&mut self) {
        self.hash.fill(0xffffffff);
    }

    pub fn clear_with_size(&mut self, hash_size: u32, index_size: u32) {
        assert!(hash_size > 0);
        assert!(hash_size.is_power_of_two());
        self.hash_size = hash_size;
        self.hash_mask = hash_size - 1;
        self.index_size = index_size;
        self.hash = vec![0xffffffff; hash_size as usize];
        self.next_index = vec![0xffffffff; index_size as usize];
    }

    pub fn resize(&mut self, new_index_size: u32) {
        if new_index_size != self.index_size {
            self.next_index.resize(new_index_size as usize, 0xffffffff);
            self.index_size = new_index_size;
        }
    }

    pub fn first(&self, key: u32) -> u32 {
        let k = key & self.hash_mask;
        self.hash[k as usize]
    }

    pub fn next(&self, index: u32) -> u32 {
        assert!(index < self.index_size);
        self.next_index[index as usize]
    }

    pub fn is_valid(&self, index: u32) -> bool {
        index != 0xffffffff
    }

    pub fn add(&mut self, key: u32, index: u32) {
        if index >= self.index_size {
            let new_size = 32.max((index + 1).next_power_of_two());
            self.resize(new_size);
        }

        let k = (key & self.hash_mask) as usize;
        self.next_index[index as usize] = self.hash[k];
        self.hash[k] = index;
    }

    pub fn remove(&mut self, key: u32, index: u32) {
        if index >= self.index_size {
            return;
        }

        let k = (key & self.hash_mask) as usize;

        if self.hash[k] == index {
            self.hash[k] = self.next_index[index as usize];
        } else {
            let mut i = self.hash[k];
            while self.is_valid(i) {
                let next = self.next_index[i as usize];
                if next == index {
                    self.next_index[i as usize] = self.next_index[index as usize];
                    break;
                }
                i = next;
            }
        }
    }
}
