//! Scoped thread pool for parallel generator calls with a bounded worker
//! count.  Built on `std::thread::scope` + a Mutex work queue — no external
//! crate.
//!
//! Order is preserved: `parallel_map` returns results in the same order as
//! the input iterator.

use std::sync::Mutex;

pub fn parallel_map<T, R, F>(items: Vec<T>, workers: usize, f: F) -> Vec<R>
where
    T: Send,
    R: Send + Default,
    F: Fn(T) -> R + Sync + Send,
{
    let workers = workers.max(1);
    let len = items.len();
    let mut results: Vec<R> = (0..len).map(|_| R::default()).collect();

    if workers == 1 || len <= 1 {
        for (i, item) in items.into_iter().enumerate() {
            results[i] = f(item);
        }
        return results;
    }

    // Queue of (index, item) pairs; workers pop from the front.
    let queue: Mutex<Vec<(usize, T)>> = Mutex::new(items.into_iter().enumerate().rev().collect());
    let results_mtx = Mutex::new(&mut results);
    let f_ref = &f;

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let q = &queue;
            let out = &results_mtx;
            scope.spawn(move || loop {
                let Some((idx, item)) = q.lock().unwrap().pop() else { return; };
                let r = f_ref(item);
                let mut slot = out.lock().unwrap();
                slot[idx] = r;
            });
        }
    });

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_order() {
        let items: Vec<u32> = (0..64).collect();
        let got = parallel_map(items.clone(), 4, |x| x * 2);
        let want: Vec<u32> = items.iter().map(|x| x * 2).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn single_worker_equivalent_to_sequential() {
        let items: Vec<u32> = (0..16).collect();
        let got = parallel_map(items, 1, |x| x + 1);
        let want: Vec<u32> = (1..17).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn zero_workers_treated_as_one() {
        let got = parallel_map(vec![10u32, 20, 30], 0, |x| x * 10);
        assert_eq!(got, vec![100, 200, 300]);
    }

    #[test]
    fn empty_input_returns_empty() {
        let got: Vec<u32> = parallel_map(Vec::<u32>::new(), 4, |x| x);
        assert!(got.is_empty());
    }
}
