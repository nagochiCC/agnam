/// Run blocking work away from the GTK main thread.
pub(crate) fn spawn_background<F, R>(work: F)
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    std::thread::spawn(work);
}
