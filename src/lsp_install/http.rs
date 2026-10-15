//! Bound idle socket reads independently of the whole-download deadline. ureq's
//! recv_body timeout covers the entire body, not each read. This small adapter
//! uses its unversioned transport API, so the dependency is pinned exactly.
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use ureq::{
    Agent, Error,
    unversioned::{
        resolver::DefaultResolver,
        transport::{
            Buffers, ConnectionDetails, Connector, DefaultConnector, NextTimeout, Transport,
        },
    },
};

pub(super) fn agent(cancel: Arc<AtomicBool>) -> Agent {
    let config = Agent::config_builder()
        .https_only(!cfg!(test))
        .max_redirects(5)
        .timeout_global(Some(super::DEADLINE))
        .timeout_resolve(Some(Duration::from_secs(5)))
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_recv_response(Some(Duration::from_secs(10)))
        .build();
    Agent::with_parts(
        config,
        DefaultConnector::default().chain(IdleLimit(cancel)),
        DefaultResolver::default(),
    )
}

#[derive(Debug)]
struct IdleLimit(Arc<AtomicBool>);
impl Connector<Box<dyn Transport>> for IdleLimit {
    type Out = Bounded;
    fn connect(
        &self,
        _: &ConnectionDetails,
        chained: Option<Box<dyn Transport>>,
    ) -> Result<Option<Bounded>, Error> {
        Ok(chained.map(|inner| Bounded {
            inner,
            cancel: self.0.clone(),
        }))
    }
}
#[derive(Debug)]
struct Bounded {
    inner: Box<dyn Transport>,
    cancel: Arc<AtomicBool>,
}
impl Bounded {
    fn timeout(&self, mut timeout: NextTimeout) -> Result<NextTimeout, Error> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Error::Io(io::Error::other("LSP installation cancelled")));
        }
        if *timeout.after > Duration::from_secs(10) {
            timeout.after = Duration::from_secs(10).into();
        }
        Ok(timeout)
    }
}
impl Transport for Bounded {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.inner.buffers()
    }
    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), Error> {
        self.inner.transmit_output(amount, self.timeout(timeout)?)
    }
    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, Error> {
        self.inner.await_input(self.timeout(timeout)?)
    }
    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }
    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}
