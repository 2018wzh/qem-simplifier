pub struct DisjointSet {
    pub parents: Vec<u32>,
}

impl DisjointSet {
    pub fn new(n: u32) -> Self {
        Self {
            parents: (0..n).collect(),
        }
    }

    pub fn reset(&mut self) {
        self.parents.clear();
    }

    pub fn add_defaulted(&mut self) {
        let id = self.parents.len() as u32;
        self.parents.push(id);
    }

    pub fn find(&mut self, mut i: u32) -> u32 {
        let mut root = i;
        while self.parents[root as usize] != root {
            root = self.parents[root as usize];
        }
        while self.parents[i as usize] != root {
            let next = self.parents[i as usize];
            self.parents[i as usize] = root;
            i = next;
        }
        root
    }

    pub fn union(&mut self, i: u32, j: u32) {
        let root_i = self.find(i);
        let root_j = self.find(j);
        if root_i != root_j {
            self.parents[root_i as usize] = root_j;
        }
    }

    // 顺序合并：将 i 的根直接指向 j，对应特定拓扑场景下的稳定合并策略。
    pub fn union_sequential(&mut self, i: u32, j: u32) {
        self.parents[i as usize] = j;
    }
}
