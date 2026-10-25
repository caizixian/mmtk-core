#![deny(unsafe_op_in_unsafe_fn)]

use super::stat::WorkerLocalStat;
use super::work_bucket::*;
use super::*;
use crate::mmtk::MMTK;
use crate::util::copy::GCWorkerCopyContext;
use crate::util::heap::layout::heap_parameters::MAX_SPACES;
use crate::util::opaque_pointer::*;
use crate::util::rust_util::phase_cell::MmtkId;
use crate::util::rust_util::worker_local::WorkerPerm;
use crate::util::ObjectReference;
use crate::vm::{Collection, GCThreadContext, VMBinding};
use atomic::Atomic;
use atomic_refcell::{AtomicRef, AtomicRefCell, AtomicRefMut};
use crossbeam::deque::{self, Stealer};
use crossbeam::queue::ArrayQueue;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// Represents the ID of a GC worker thread.
pub type ThreadId = usize;

thread_local! {
    /// Current worker's ordinal
    static WORKER_ORDINAL: Atomic<ThreadId> = const { Atomic::new(ThreadId::MAX) };
}

/// Get current worker ordinal. Return `None` if the current thread is not a worker.
pub fn current_worker_ordinal() -> Option<ThreadId> {
    let ordinal = WORKER_ORDINAL.with(|x| x.load(Ordering::Relaxed));
    if ordinal == ThreadId::MAX {
        None
    } else {
        Some(ordinal)
    }
}

/// The struct has one instance per worker, but is shared between workers via the scheduler
/// instance.  This structure is used for communication between workers, e.g. adding designated
/// work packets, stealing work packets from other workers, and collecting per-worker statistics.
pub struct GCWorkerShared<VM: VMBinding> {
    /// Worker-local statistics data.
    stat: AtomicRefCell<WorkerLocalStat<VM>>,
    /// Accumulated bytes for live objects in this GC. When each worker scans
    /// objects, we increase the live bytes. We get this value from each worker
    /// at the end of a GC, and reset this counter.
    /// The live bytes are stored in an array. The index is the index from the space descriptor.
    pub live_bytes_per_space: AtomicRefCell<[usize; MAX_SPACES]>,
    /// A queue of GCWork that can only be processed by the owned thread.
    pub designated_work: ArrayQueue<Box<dyn GCWork<VM>>>,
    /// Handle for stealing packets from the current worker
    pub stealer: Option<Stealer<Box<dyn GCWork<VM>>>>,
}

impl<VM: VMBinding> GCWorkerShared<VM> {
    /// Create a new `GCWorkerShared` instance, optionally with a `stealer` handle that other
    /// workers can use to steal work packets from this worker's local queue.
    pub fn new(stealer: Option<Stealer<Box<dyn GCWork<VM>>>>) -> Self {
        Self {
            stat: Default::default(),
            live_bytes_per_space: AtomicRefCell::new([0; MAX_SPACES]),
            designated_work: ArrayQueue::new(16),
            stealer,
        }
    }

    pub(crate) fn increase_live_bytes(
        live_bytes_per_space: &mut [usize; MAX_SPACES],
        object: ObjectReference,
    ) {
        use crate::mmtk::VM_MAP;
        use crate::vm::object_model::ObjectModel;

        // The live bytes of the object
        let bytes = VM::VMObjectModel::get_current_size(object);
        // Get the space index from descriptor
        let space_descriptor = VM_MAP.get_descriptor_for_address(object.to_raw_address());
        if space_descriptor != crate::util::heap::space_descriptor::SpaceDescriptor::UNINITIALIZED {
            let space_index = space_descriptor.get_index();
            debug_assert!(
                space_index < MAX_SPACES,
                "Space index {} is not in the range of [0, {})",
                space_index,
                MAX_SPACES
            );
            // Accumulate the live bytes for the index
            live_bytes_per_space[space_index] += bytes;
        }
    }
}

/// A GC worker.  This part is privately owned by a worker thread.
pub struct GCWorker<VM: VMBinding> {
    /// The VM-specific thread-local state of the GC thread.
    pub tls: VMWorkerThread,
    /// The ordinal of the worker, numbered from 0 to the number of workers minus one.
    pub ordinal: ThreadId,
    /// Pre-minted instance-branded capability proving exclusive worker-local access for `(mmtk.id, ordinal)`.
    pub(crate) perm: WorkerPerm,
    /// The reference to the scheduler.
    scheduler: Arc<GCWorkScheduler<VM>>,
    /// The copy context, used to implement copying GC.
    copy: GCWorkerCopyContext<VM>,
    /// True once `GCWorker::run` has initialized `self.copy` via `create_gc_worker_context`.
    copy_initialized: bool,
    /// The reference to the MMTk instance.
    pub mmtk: &'static MMTK<VM>,
    /// Reference to the shared part of the GC worker.  It is used for synchronization.
    pub shared: Arc<GCWorkerShared<VM>>,
    /// Local work packet queue.
    pub local_work_buffer: deque::Worker<Box<dyn GCWork<VM>>>,
}

// Error message for borrowing `GCWorkerShared::stat`.
const STAT_BORROWED_MSG: &str = "GCWorkerShared.stat is already borrowed.  This may happen if \
    the mutator calls harness_begin or harness_end while the GC is running.";

impl<VM: VMBinding> GCWorkerShared<VM> {
    /// Immutably borrow this worker's local statistics.
    pub fn borrow_stat(&self) -> AtomicRef<'_, WorkerLocalStat<VM>> {
        self.stat.try_borrow().expect(STAT_BORROWED_MSG)
    }

    /// Mutably borrow this worker's local statistics.
    pub fn borrow_stat_mut(&self) -> AtomicRefMut<'_, WorkerLocalStat<VM>> {
        self.stat.try_borrow_mut().expect(STAT_BORROWED_MSG)
    }
}

/// A special error type that indicate a worker should exit.
/// This may happen if the VM needs to fork and asks workers to exit.
#[derive(Debug)]
pub(crate) struct WorkerShouldExit;

/// The result type of `GCWorker::pool`.
/// Too many functions return `Option<Box<dyn GCWork<VM>>>`.  In most cases, when `None` is
/// returned, the caller should try getting work packets from another place.  To avoid confusion,
/// we use `Err(WorkerShouldExit)` to clearly indicate that the worker should exit immediately.
pub(crate) type PollResult<VM> = Result<Box<dyn GCWork<VM>>, WorkerShouldExit>;

impl<VM: VMBinding> GCWorker<VM> {
    pub(crate) fn new(
        mmtk: &'static MMTK<VM>,
        ordinal: ThreadId,
        perm: WorkerPerm,
        scheduler: Arc<GCWorkScheduler<VM>>,
        shared: Arc<GCWorkerShared<VM>>,
        local_work_buffer: deque::Worker<Box<dyn GCWork<VM>>>,
    ) -> Self {
        assert_eq!(perm.instance_id(), mmtk.id, "GCWorker::new MmtkId mismatch");
        assert_eq!(
            perm.ordinal(),
            ordinal,
            "GCWorker::new WorkerPerm ordinal mismatch"
        );
        Self {
            tls: VMWorkerThread(VMThread::UNINITIALIZED),
            ordinal,
            perm,
            // We will set this later
            copy: GCWorkerCopyContext::new_non_copy(),
            copy_initialized: false,
            scheduler,
            mmtk,
            shared,
            local_work_buffer,
        }
    }

    const LOCALLY_CACHED_WORK_PACKETS: usize = 16;

    /// Add a boxed work packet to the work queue, in the given bucket.
    /// Like [`GCWorker::add_work`], but the work packet is already boxed.
    pub fn add_boxed_work(&mut self, bucket: WorkBucketStage, work: Box<dyn GCWork<VM>>) {
        if !self.scheduler().work_buckets[bucket].is_open()
            || self.local_work_buffer.len() >= Self::LOCALLY_CACHED_WORK_PACKETS
        {
            self.scheduler.work_buckets[bucket].add_boxed(work);
            return;
        }
        self.local_work_buffer.push(work);
    }

    /// Add a work packet to the work queue.
    /// If the bucket is open, the packet will be pushed to the local queue, otherwise it will be
    /// pushed to the global bucket.
    pub fn add_work(&mut self, bucket: WorkBucketStage, work: impl GCWork<VM>) {
        if !self.scheduler().work_buckets[bucket].is_open()
            || self.local_work_buffer.len() >= Self::LOCALLY_CACHED_WORK_PACKETS
        {
            self.scheduler.work_buckets[bucket].add(work);
            return;
        }
        self.local_work_buffer.push(Box::new(work));
    }

    /// Get the scheduler. There is only one scheduler per MMTk instance.
    pub fn scheduler(&self) -> &GCWorkScheduler<VM> {
        &self.scheduler
    }

    /// Get a mutable reference of the copy context for this worker.
    pub fn get_copy_context_mut(&mut self) -> &mut GCWorkerCopyContext<VM> {
        &mut self.copy
    }

    pub(crate) fn is_copy_context_initialized(&self) -> bool {
        self.copy_initialized
    }

    /// Poll a ready-to-execute work packet in the following order:
    ///
    /// 1. Any packet that should be processed only by this worker.
    /// 2. Poll from the local work queue.
    /// 3. Poll from open global work-buckets
    /// 4. Steal from other workers
    fn poll(&mut self) -> PollResult<VM> {
        if let Some(work) = self.shared.designated_work.pop() {
            return Ok(work);
        }

        if let Some(work) = self.local_work_buffer.pop() {
            return Ok(work);
        }

        if let Some(work) = self.scheduler.poll_schedulable_work(self) {
            return Ok(work);
        }

        let scheduler = self.scheduler.clone();
        scheduler.poll_slow(self)
    }

    /// Entry point of the worker thread.
    ///
    /// This function will resolve thread affinity, if it has been specified by the user.
    ///
    /// Each worker will keep polling and executing work packets in a loop.  It runs until the
    /// worker is requested to exit.  Currently a worker may exit after
    /// [`crate::mmtk::MMTK::prepare_to_fork`] is called.
    ///
    /// Arguments:
    /// * `tls`: The VM-specific thread-local storage for this GC worker thread.
    /// * `mmtk`: A reference to an MMTk instance.
    pub fn run(mut self: Box<Self>, tls: VMWorkerThread, mmtk: &'static MMTK<VM>) {
        assert_eq!(
            self.mmtk.id, mmtk.id,
            "GCWorker::run called with a different MMTK instance ID than the one that owns this worker"
        );
        assert_eq!(
            self.perm.instance_id(),
            mmtk.id,
            "GCWorker::run WorkerPerm MmtkId mismatch"
        );
        assert!(
            std::ptr::eq(self.mmtk, mmtk),
            "GCWorker::run called with a different MMTK instance than the one that owns this worker"
        );
        probe!(mmtk, gcworker_run);
        debug!(
            "Worker started. ordinal: {}, {}",
            self.ordinal,
            crate::util::rust_util::debug_process_thread_id(),
        );
        WORKER_ORDINAL.with(|x| x.store(self.ordinal, Ordering::SeqCst));
        self.scheduler.resolve_affinity(self.ordinal);
        self.tls = tls;
        self.copy = crate::plan::create_gc_worker_context(tls, mmtk);
        self.copy_initialized = true;
        loop {
            // Instead of having work_start and work_end tracepoints, we have
            // one tracepoint before polling for more work and one tracepoint
            // before executing the work.
            // This allows measuring the distribution of both the time needed
            // poll work (between work_poll and work), and the time needed to
            // execute work (between work and next work_poll).
            // If we have work_start and work_end, we cannot measure the first
            // poll.
            probe!(mmtk, work_poll);
            let Ok(mut work) = self.poll() else {
                // The worker is asked to exit.  Break from the loop.
                break;
            };
            // probe! expands to an empty block on unsupported platforms
            #[allow(unused_variables)]
            let typename = work.get_type_name();

            #[cfg(feature = "bpftrace_workaround")]
            // Workaround a problem where bpftrace script cannot see the work packet names,
            // by force loading from the packet name.
            // See the "Known issues" section in `tools/tracing/timeline/README.md`
            std::hint::black_box(typename.as_bytes().first().copied());

            probe!(mmtk, work, typename.as_ptr(), typename.len());
            debug_assert!(
                self.scheduler().is_worker_active(self.ordinal),
                "Worker {} is executing {} while inactive.",
                self.ordinal,
                typename
            );
            work.do_work_with_stat(&mut self, mmtk);
        }
        debug!(
            "Worker exiting. ordinal: {}, {}",
            self.ordinal,
            crate::util::rust_util::debug_process_thread_id(),
        );
        probe!(mmtk, gcworker_exit);

        mmtk.scheduler.surrender_gc_worker(self);
    }
}

/// Stateful part of [`WorkerGroup`].
enum WorkerCreationState<VM: VMBinding> {
    /// The initial state.  `GCWorker` structs have not been created and GC worker threads have not
    /// been spawn.
    Initial {
        /// The local work queues for to-be-created workers.
        local_work_queues: Vec<deque::Worker<Box<dyn GCWork<VM>>>>,
        /// Pre-minted unique `(instance_id, ordinal)` capabilities consumed once in `create_workers`.
        perms: Vec<WorkerPerm>,
    },
    /// All worker threads are spawn and running.  `GCWorker` structs have been transferred to
    /// worker threads.
    Spawned,
    /// Worker threads are stopping, or have already stopped, for forking. Instances of `GCWorker`
    /// structs are collected here to be reused when GC workers are respawn.
    Surrendered {
        /// Type-erased `Box<GCWorker<VM>>` instances not currently owned by active GC worker
        /// threads. Erasing `GCWorker<VM>` to `dyn Any + Send` breaks the structural auto-trait
        /// cycle `WorkerGroup -> GCWorker -> Arc<GCWorkScheduler> -> Arc<WorkerGroup>` while still
        /// requiring the compiler to prove `GCWorker<VM>: Send` at coercion time (`0` `unsafe`).
        workers: Vec<Box<dyn std::any::Any + Send>>,
    },
}

/// A worker group to manage all the GC workers.
pub(crate) struct WorkerGroup<VM: VMBinding> {
    /// Shared worker data
    pub workers_shared: Vec<Arc<GCWorkerShared<VM>>>,
    /// The stateful part.  `None` means state transition is underway.
    state: Mutex<Option<WorkerCreationState<VM>>>,
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    #[allow(dead_code)]
    fn verify_worker_group_send_sync<VM: VMBinding>() {
        assert_send_sync::<WorkerGroup<VM>>();
    }
};

impl<VM: VMBinding> WorkerGroup<VM> {
    /// Create a WorkerGroup with the pre-minted `WorkerPerm` capabilities from `WorkerMonitor::new`.
    pub fn new(instance_id: MmtkId, num_workers: usize, perms: Vec<WorkerPerm>) -> Arc<Self> {
        assert_eq!(
            perms.len(),
            num_workers,
            "WorkerGroup::new WorkerPerm count mismatch"
        );
        for (ordinal, perm) in perms.iter().enumerate() {
            assert_eq!(
                perm.instance_id(),
                instance_id,
                "WorkerGroup::new WorkerPerm MmtkId mismatch"
            );
            assert_eq!(
                perm.ordinal(),
                ordinal,
                "WorkerGroup::new WorkerPerm ordinal mismatch"
            );
        }
        let local_work_queues = (0..num_workers)
            .map(|_| deque::Worker::new_fifo())
            .collect::<Vec<_>>();

        let workers_shared = (0..num_workers)
            .map(|i| {
                Arc::new(GCWorkerShared::<VM>::new(Some(
                    local_work_queues[i].stealer(),
                )))
            })
            .collect::<Vec<_>>();

        Arc::new(Self {
            workers_shared,
            state: Mutex::new(Some(WorkerCreationState::Initial {
                local_work_queues,
                perms,
            })),
        })
    }

    /// Spawn GC worker threads for the first time.
    pub fn initial_spawn(&self, tls: VMThread, mmtk: &'static MMTK<VM>) {
        let mut state = self.state.lock().unwrap();

        let WorkerCreationState::Initial {
            local_work_queues,
            perms,
        } = state.take().unwrap()
        else {
            panic!("GCWorker structs have already been created");
        };

        let workers = self.create_workers(local_work_queues, perms, mmtk);
        self.spawn(workers, tls);

        *state = Some(WorkerCreationState::Spawned);
    }

    /// Respawn GC threads after stopping for forking.
    pub fn respawn(&self, tls: VMThread) {
        let mut state = self.state.lock().unwrap();

        let WorkerCreationState::Surrendered { workers } = state.take().unwrap() else {
            panic!("GCWorker structs have not been created, yet.");
        };

        let workers: Vec<Box<GCWorker<VM>>> = workers
            .into_iter()
            .map(|w| {
                w.downcast::<GCWorker<VM>>()
                    .unwrap_or_else(|_| unreachable!("Surrendered worker must be GCWorker<VM>"))
            })
            .collect();

        self.spawn(workers, tls);

        *state = Some(WorkerCreationState::Spawned)
    }

    /// Create `GCWorker` instances.
    #[allow(clippy::vec_box)] // See `WorkerCreationState::Surrendered`.
    fn create_workers(
        &self,
        local_work_queues: Vec<deque::Worker<Box<dyn GCWork<VM>>>>,
        perms: Vec<WorkerPerm>,
        mmtk: &'static MMTK<VM>,
    ) -> Vec<Box<GCWorker<VM>>> {
        debug!("Creating GCWorker instances...");

        assert_eq!(self.workers_shared.len(), local_work_queues.len());
        assert_eq!(self.workers_shared.len(), perms.len());

        // Each `GCWorker` instance corresponds to a `GCWorkerShared` and unique `WorkerPerm` at the same index.
        let workers = local_work_queues
            .into_iter()
            .zip(perms)
            .zip(self.workers_shared.iter())
            .enumerate()
            .map(|(ordinal, ((queue, perm), shared))| {
                Box::new(GCWorker::new(
                    mmtk,
                    ordinal,
                    perm,
                    mmtk.scheduler.clone(),
                    shared.clone(),
                    queue,
                ))
            })
            .collect::<Vec<_>>();

        debug!("Created {} GCWorker instances.", workers.len());
        workers
    }

    /// Spawn all the worker threads
    #[allow(clippy::vec_box)] // See `WorkerCreationState::Surrendered`.
    fn spawn(&self, workers: Vec<Box<GCWorker<VM>>>, tls: VMThread) {
        debug!(
            "Spawning GC workers.  {}",
            crate::util::rust_util::debug_process_thread_id(),
        );

        // We transfer the ownership of each `GCWorker` instance to a GC thread.
        for worker in workers {
            VM::VMCollection::spawn_gc_thread(tls, GCThreadContext::<VM>::Worker(worker));
        }

        debug!(
            "Spawned {} worker threads.  {}",
            self.worker_count(),
            crate::util::rust_util::debug_process_thread_id(),
        );
    }

    /// Prepare the buffer for workers to surrender their `GCWorker` structs.
    pub fn prepare_surrender_buffer(&self) {
        let mut state = self.state.lock().unwrap();
        assert!(matches!(*state, Some(WorkerCreationState::Spawned)));

        *state = Some(WorkerCreationState::Surrendered {
            workers: Vec::with_capacity(self.worker_count()),
        })
    }

    /// Return the `GCWorker` struct to the worker group.
    /// This function returns `true` if all workers returned their `GCWorker` structs.
    pub fn surrender_gc_worker(&self, worker: Box<GCWorker<VM>>) -> bool {
        let mut state = self.state.lock().unwrap();
        let WorkerCreationState::Surrendered { ref mut workers } = state.as_mut().unwrap() else {
            panic!("GCWorker structs have not been created, yet.");
        };
        let ordinal = worker.ordinal;
        workers.push(worker);
        trace!(
            "Worker {} surrendered. ({}/{})",
            ordinal,
            workers.len(),
            self.worker_count()
        );
        workers.len() == self.worker_count()
    }

    /// Get the number of workers in the group
    pub fn worker_count(&self) -> usize {
        self.workers_shared.len()
    }

    /// Return true if there're any pending designated work
    pub fn has_designated_work(&self) -> bool {
        self.workers_shared
            .iter()
            .any(|w| !w.designated_work.is_empty())
    }

    /// Get the live bytes data from the worker, and clear the local data.
    pub fn get_and_clear_worker_live_bytes(&self) -> [usize; MAX_SPACES] {
        let mut ret = [0; MAX_SPACES];
        self.workers_shared.iter().for_each(|w| {
            let mut live_bytes_per_space = w.live_bytes_per_space.borrow_mut();
            for (idx, val) in live_bytes_per_space.iter_mut().enumerate() {
                ret[idx] += *val;
                *val = 0;
            }
        });
        ret
    }
}
