use std::collections::{HashMap};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
struct CacheEntity<T> {
    key: String,
    value: Arc<T>,
    exp: u128,
    lru_prev: Option<NonNull<Self>>,
    lru_next: Option<NonNull<Self>>,
    exp_prev: Option<NonNull<Self>>,
    exp_next: Option<NonNull<Self>>,
}

pub struct LocalCache<T>(Mutex<InnerLocalCache<T>>);

struct InnerLocalCache<T> {
    max_numbers: usize,
    max_age_ns: u128,
    lru_head: Option<NonNull<CacheEntity<T>>>,
    lru_tail: Option<NonNull<CacheEntity<T>>>,
    exp_head: Option<NonNull<CacheEntity<T>>>,
    exp_tail: Option<NonNull<CacheEntity<T>>>,
    map: HashMap<String, NonNull<CacheEntity<T>>>,
}

impl<T> InnerLocalCache<T> {
    fn new(max_numbers: usize, max_age_ns: u128) -> Self {
        Self {
            max_numbers,
            max_age_ns,
            lru_head: None,
            lru_tail: None,
            exp_head: None,
            exp_tail: None,
            map: Default::default(),
        }
    }
    unsafe fn get(&mut self, key: &String) -> Option<Arc<T>> {
        let value = self.map.get(key);
        if value.is_none() {
            return None;
        }
        let mut non_null = value.unwrap().clone();
        let entity = non_null.as_mut();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        if now > entity.exp {
            return None;
        }
        self.remove_lru(non_null.clone());
        if self.lru_head.is_some() {
            let mut old_lru_head = self.lru_head.unwrap();
            old_lru_head.as_mut().lru_prev = Some(non_null.clone());
            entity.lru_next = self.lru_head.clone();
        }
        if self.lru_tail.is_none() {
            self.lru_tail = Some(non_null.clone());
        }
        self.lru_head = Some(non_null);

        Some(entity.value.clone())
    }
    unsafe fn put(&mut self, key: String, value: Arc<T>) {
        self.remove(&key);

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        self.clean(now);

        let exp = now + self.max_age_ns;

        let new_entity = Box::new(CacheEntity {
            key: key.clone(),
            value,
            exp,
            lru_prev: None,
            lru_next: self.lru_head.clone(),
            exp_prev: self.exp_tail.clone(),
            exp_next: None,
        });
        let mut cur_entity = NonNull::from(Box::leak(new_entity));

        let _ = self.map.insert(key, cur_entity.clone());
        let old_lru_head = self.lru_head.replace(cur_entity.clone());
        let old_exp_head = self.exp_head.replace(cur_entity.clone());
        match self.map.len() {
            0 | 1 => {
                let _ = self.lru_tail.replace(cur_entity.clone());
                let _ = self.exp_tail.replace(cur_entity);
                return;
            }
            _ => {
                let mut old_lru_head = old_lru_head.unwrap();
                let inner_old_lru_head = old_lru_head.as_mut();
                inner_old_lru_head.lru_prev = Some(cur_entity.clone());
                cur_entity.as_mut().lru_next = Some(old_lru_head.clone());

                let mut old_exp_head = old_exp_head.unwrap();
                let inner_old_exp_head = old_exp_head.as_mut();
                inner_old_exp_head.exp_prev = Some(cur_entity.clone());
                cur_entity.as_mut().exp_next = Some(old_exp_head.clone());
            }
        }
    }

    unsafe fn clean(&mut self, now: u128) {
        if self.map.len() < self.max_numbers {
            return;
        }
        let mut cur = self.exp_tail.clone();
        while cur.is_some() {
            let e = cur.unwrap();
            let b = e.as_ref();
            if b.exp > now {
                break;
            }
            self.remove(&b.key);
            cur = b.exp_prev.clone();
        }
        while self.map.len() >= self.max_numbers {
            let key = self.lru_tail.map(|e| e.as_ref().key.clone()).unwrap();
            self.remove(&key);
        }
    }

    unsafe fn remove(&mut self, key: &String) {
        let old = self.map.remove(key);
        if old.is_none() {
            return;
        }

        let old = old.unwrap();
        self.remove_lru(old.clone());
        self.remove_exp(old.clone());
        let _ = Box::from_raw(old.as_ptr());
    }

    unsafe fn remove_lru(&mut self, mut non_null: NonNull<CacheEntity<T>>) {
        let entity = non_null.as_mut();
        let key = &entity.key;
        entity.lru_prev.clone().inspect(|e| {
            e.clone().as_mut().lru_next = entity.lru_next.clone()
        });
        entity.lru_next.clone().inspect(|e| {
            e.clone().as_mut().lru_prev = entity.lru_prev.clone()
        });


        if let Some(lru_head) = self.lru_head.clone() {
            if &lru_head.as_ref().key == key {
                self.lru_head = entity.lru_next.clone();
            }
        }
        if let Some(lru_tail) = self.lru_tail.clone() {
            if &lru_tail.as_ref().key == key {
                self.lru_tail = entity.lru_prev.clone();
            }
        }
    }
    unsafe fn remove_exp(&mut self, mut non_null: NonNull<CacheEntity<T>>) {
        let entity = non_null.as_mut();
        let key = &entity.key;
        entity.exp_prev.clone().inspect(|e| {
            e.clone().as_mut().exp_next = entity.exp_next.clone()
        });
        entity.exp_next.clone().inspect(|e| {
            e.clone().as_mut().exp_prev = entity.exp_prev.clone()
        });


        if let Some(exp_head) = self.exp_head.clone() {
            if &exp_head.as_ref().key == key {
                self.exp_head = entity.exp_next.clone();
            }
        }
        if let Some(exp_tail) = self.exp_tail.clone() {
            if &exp_tail.as_ref().key == key {
                self.exp_tail = entity.exp_prev.clone();
            }
        }
    }
}

impl<T> LocalCache<T> {
    pub fn new(max_numbers: usize, max_age_secs: u64) -> Self {
        Self(Mutex::new(InnerLocalCache::new(max_numbers, Duration::from_secs(max_age_secs).as_nanos())))
    }
    pub fn get(&self, key: &String) -> Option<Arc<T>> {
        let mut local_cache = self.0.lock().unwrap();
        unsafe { local_cache.get(key) }
    }

    pub fn put(&self, key: String, value: Arc<T>) {
        let mut local_cache = self.0.lock().unwrap();
        unsafe { local_cache.put(key, value) }
    }
}

#[test]
fn test() {
    println!("Hello, world!");
    let local_cache: LocalCache<String> = LocalCache::new(1, 360);

    assert_eq!(None, local_cache.get(&"x".to_string()));
    local_cache.put(String::from("x"), Arc::new(String::from("abc")));
    println!("{:?}", local_cache.get(&"x".to_string()));

    local_cache.put(String::from("x"), Arc::new(String::from("abc")));
    println!("{:?}", local_cache.get(&"x".to_string()));

    assert_eq!(None, local_cache.get(&"y".to_string()));
    local_cache.put(String::from("y"), Arc::new(String::from("123")));
    println!("{:?}", local_cache.get(&"y".to_string()));

    assert_eq!(None, local_cache.get(&"x".to_string()));
}
