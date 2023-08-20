use std::sync::Arc;

use super::*;
use tokio::{
    runtime::Builder,
    sync::mpsc::{error::TryRecvError, unbounded_channel, UnboundedReceiver, UnboundedSender},
    task::LocalSet,
};

enum WorkerInputEvent {
    Create(Arc<LogicArgs>),
}

pub struct Worker {
    inputs: UnboundedSender<WorkerInputEvent>,
}

impl Worker {
    pub fn new(module_resolver: Arc<ModuleResolver>) -> Self {
        let (sender, recv) = unbounded_channel();
        WorkerImpl::spawn(module_resolver, recv);
        Worker { inputs: sender }
    }

    pub fn create_runtime(&mut self, args: Arc<LogicArgs>) -> EResult<RuntimeConnection> {
        // Extend this to load balance.
        self.inputs.send(WorkerInputEvent::Create(args))?;
        Ok(RuntimeConnection)
    }
}

struct WorkerImpl {
    control_src: UnboundedReceiver<WorkerInputEvent>,
    module_resolver: Arc<ModuleResolver>,
    runtimes: Vec<Runtime>,
}

impl WorkerImpl {
    fn spawn(
        module_resolver: Arc<ModuleResolver>,
        control_src: UnboundedReceiver<WorkerInputEvent>,
    ) {
        std::thread::spawn(|| {
            let local = LocalSet::new();
            local.spawn_local(
                WorkerImpl {
                    control_src,
                    module_resolver,
                    runtimes: default(),
                }
                .main(),
            );
            Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(local)
        });
    }

    async fn main(mut self) {
        loop {
            match self.control_src.try_recv() {
                Ok(WorkerInputEvent::Create(arg)) => {
                    if let Ok(runtime) = Runtime::new(self.module_resolver.clone(), arg).await {
                        self.runtimes.push(runtime);
                    }
                }
                Err(TryRecvError::Disconnected) => todo!("shutdown"),
                Err(TryRecvError::Empty) => {} // nothing to change.
            }

            for runtime in &mut self.runtimes {
                runtime.run_event_loop().await;
            }
        }
    }
}
