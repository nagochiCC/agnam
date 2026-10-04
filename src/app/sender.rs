use super::Msg;

#[derive(Clone)]
pub(crate) struct AppSender(async_channel::Sender<Msg>);

impl AppSender {
    pub(crate) fn channel() -> (Self, async_channel::Receiver<Msg>) {
        let (sender, receiver) = async_channel::unbounded();
        (Self(sender), receiver)
    }

    pub(crate) fn input(&self, message: Msg) {
        let _ = self.0.try_send(message);
    }

    pub(crate) fn send(&self, message: Msg) -> bool {
        self.0.try_send(message).is_ok()
    }

    pub(crate) fn close(&self) {
        self.0.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_sender_rejects_messages() {
        let (sender, _receiver) = AppSender::channel();
        sender.close();

        assert!(!sender.send(Msg::OpenFile));
    }
}
