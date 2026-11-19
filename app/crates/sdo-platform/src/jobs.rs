//! A bounded queue for short preparation jobs. GPU objects stay on their owning thread in browsers.
use std::sync::mpsc;
pub struct Jobs<I, O> {
    #[cfg(not(target_arch = "wasm32"))]
    input: mpsc::SyncSender<I>,
    #[cfg(target_arch = "wasm32")]
    submit: Box<dyn Fn(I) -> anyhow::Result<()>>,
    output: mpsc::Receiver<O>,
}
impl<I: 'static, O: 'static> Jobs<I, O> {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(capacity: usize, mut run: impl FnMut(I) -> O + Send + 'static) -> Self
    where
        I: Send,
        O: Send,
    {
        let (input, rx) = mpsc::sync_channel(capacity);
        let (tx, output) = mpsc::sync_channel(capacity);
        std::thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                if tx.send(run(job)).is_err() {
                    break;
                }
            }
        });
        Self { input, output }
    }
    #[cfg(target_arch = "wasm32")]
    pub fn new(capacity: usize, run: impl FnMut(I) -> O + 'static) -> Self
    where
        I: Clone,
    {
        use std::{
            cell::{Cell, RefCell},
            rc::Rc,
        };
        let (tx, output) = mpsc::channel();
        let run = Rc::new(RefCell::new(run));
        let pending = Rc::new(Cell::new(0));
        let submit = Box::new(move |job: I| {
            anyhow::ensure!(pending.get() < capacity, "Asset queue is full");
            pending.set(pending.get() + 1);
            let pending = pending.clone();
            let run = run.clone();
            let tx = tx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = crate::assets::hydrate(|| run.borrow_mut()(job.clone())).await;
                let _ = tx.send(result);
                pending.set(pending.get() - 1);
            });
            Ok(())
        });
        Self { submit, output }
    }
    pub fn send(&self, job: I) -> anyhow::Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.input
                .try_send(job)
                .map_err(|_| anyhow::anyhow!("Asset queue unavailable"))
        }
        #[cfg(target_arch = "wasm32")]
        {
            (self.submit)(job)
        }
    }
    pub fn poll(&self) -> Result<O, mpsc::TryRecvError> {
        self.output.try_recv()
    }
}
