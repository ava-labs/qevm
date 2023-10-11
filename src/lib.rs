//! # qEVM: Quick/Quaint/Quality Ethereum Virtual Machine
//!
//! - In short, qEVM = EVM - janky stuff + (standalone) modularity with simple interface + cool features.
//!
//! - qEVM is a clean rework of EVM from scratch in Rust, with a clear mind of supporting its
//!   current instruction set to be able to run most of the smart contracts compiled by solidity. It
//!   is what I would have done if I were implementing the first EVM for the Ethereum client. It
//!   should be reusable at different levels of abstraction and allow easy customization (storage,
//!   checkpoints, caching policies, etc.) to adapt to a blockchain platform.
//!
//! - We hope to eventually replace the old EVM in the future if this initiative turns out to work well.
//!
//! # Design Philosophy & Overview
//! This qEVM crate consists of several modules that could be utilized by the developers in a
//! standalone kind of fashion. We intend to keep the each important component clear in its
//! functionality and contains only what it provides.
//!
//! For instance, by default feature set, the crate will only provide the following crucial
//! functionalities to setup a basic execution environment:
//! - [core]: a work thread that interprets a transaction in a non-recursive fashion.
//! - [processor]: offers a set of scheduling logics to schedule/submit transaction tasks to [core]. (for a geth equivalent, see [processor::run_single_tx](processor/fn.run_single_tx.html))
//! - [state]: implements the in-memory state management for fast interpretation and defines the interface for persistent store/caching that works with the in-memory states.
//! - [common]: some useful basic types used by qEVM.
//!
//! A developer could simply run a transaction with only a few steps:
//!
//! - Spawn the a single core executor by [Core::new](core/struct.Core.html#method.new).
//! - Set up the execution context by [TxExecState::new](core/enum.TxExecState.html#method.new).
//! - Use the existing single-thread, single transaction scheduler [processor::run_single_tx](processor/fn.run_single_tx.html) to finish the execution of the transaction given the context and core.
//!
//! Done! You have a working example. (See `examples/hello-world.rs` for a concrete example).
//!
//! For a realistic system that accepts basic Ethereum RPC requests and parses transactions, you
//! can enable `rpc` and `tx` features respectively to utilize the additional functionalities (and
//! `chain` feature to use helper abstraction for a in-memory chain and blocks). See
//! `examples/demo.rs` for a more full-fledge example that implements a persistent, working chain
//! that could be communicated via Metamask/Remix.
//!
//! Of course, for a high performance system, there are many details where you could introduce
//! optimizations and thanks to qEVM's abstraction, you can customize your system at ease.
//!
//! # On the Data Flow
//! In a nutshell, no matter how sophisticated it is, an EVM system could be viewed as simple as
//! the following major data flow:
//!
//! - The execution looks up some historical state from the memory or disk (Read),
//! - makes changes in-memory (Modify),
//! - and write back changes to the disk to persist the new state (when necessary). (Write)
//!
//! This RMW loop goes on and on for each transaction on the chain. For an "archival" node, all
//! intermediate states may be persisted, whereas for a more "lightweight" validator, it only needs
//! to persist *some* of the states, and definitely the most recent state. Regardless of the
//! specific type of demands, the RMW loop still exists and state changes can't always be volatile.
//!
//! By such simple observation, qEVM uses a very simple yet powerful abstraction to describe each
//! part of the data flow so it opens to extensive future opportunity of optimizations and system
//! customizations.
//!
//! To work with the interpreter ([Core](core/struct.Core.html)), [WorldState](core/trait.WorldState.html) is the direct
//! abstraction of a world state of EVM. It resembles the `StateDB` and `stateObject` in geth but
//! gets much cleaner in a single, flat interface. Due to the asymmetry of reads and writes in the
//! RMW loop (usually reads may halt the execution because something has to be brought from the
//! disk, whereas writes could be buffered in memory and scheduled later to the disk as an atomic
//! batch), we breakdown
//! `WorldState` into two halves:
//!
//! - [WorldStateR](core/trait.WorldStateR.html): read-only part of the world state, one can perform read operations to the
//! world state, with any implementation of this interface (trait). That said, both the in-memory
//! state in work and the persistent store can implement the same interface. Furthermore, a cache
//! layer that sits in-between can do it as well. As later introduced in *state versioning*, one can
//! plug and play these layers in an ergonomic way.
//!
//! - [WorldStateW](core/trait.WorldStateW.html): write-only part of the world state. An in-memory working state should support
//! implement this interface so the interpreter quickly writes to the state without waiting for the disk action
//! after each operation. Again, this will be revisited when talk about state versioning. Unlike
//! the in-memory state implementation, a persistent store may not want to implement this interface
//! because the granularity is too small and writes could overlap and cancel each other out so it
//! is better to group them in batches when the execution is finished. Thus,
//! there is [WorldStateStore](state/trait.WorldStateStore.html) interface that accounts for this.
//!
//! Additionally, `WorldState` requires `snapshot()` and `rollback()` implementation as the
//! execution could be reverted, and [state::MemStateAuto](state/struct.MemStateAuto.html#method.new) offers a fast in-memory state implementation based
//! on state versioning.
//!
//! # State Versioning
//!
//! Unlike geth, we do not journal each write induced by the execution in a separate in-memory log
//! and play it backwards to undo the changes when the execution reverts. Instead, we let all
//! writes go *in-place* to an existing state by maintain the *deltas*, without any extra journaling; and
//! once a snapshot is taken, a new state builds on top of the existing state to record the new
//! deltas so the old state could be shared by multiple alternative new states that are based on
//! it. We adopt this because of the following advantages:
//!
//! - Less redundancy in operations: without extra journaling, when execution is not reverted, all
//! changes are made directly to the in-memory state delta set, which could be implemented by a
//! flat hash map. No extra cost is induced.
//!
//! - Fast snapshot: when making a snapshot, one just needs to overlay another state
//! ("revision", see [state::MemStateRev](state/struct.MemStateRev.html)) on top of the existing
//! state, and that's it.
//!
//! - Zero-time rollback: one can simply discard the inappropriate state that's ahead, and reuse
//! the old state revision, so there is no need to play back the journal and undo the operations.
//! There is no cost in the rollback. The chain of overlays itself is the implicit "journal".
//!
//! - Ergonomic design: the versioning style streamlines with the multi-layer
//! storage implemenation. Imagine when a lookup happens, it first searches for the tip overlay to
//! see if any delta has that value. When it misses, it will dive deeper to the next (older)
//! overlay and so on, finally, the last "overlay" could default to an external storage source
//! (anything that implements `WorldStateR`, which could be a cache, or a persistent store). This
//! way, we allow an easy way to not only chain the revisions internally for the in-memory state,
//! but also offers a natural, fast access to the underlying storage system.
//!
//! - The only catch: of course, there is no free lunch. The tradeoff we make here is the lookup time
//! for a long-overlayed state increases because it walks backwards on the chain of
//! overlays until it hits the first recorded value. However, obviously, the chain length is only
//! grown when one needs to take a snapshot. Snapshots don't frequently appear in the execution, and
//! when they do, one can always squash ([MemStateShared::consolidate](state/struct.MemStateShared.html#method.consolidate))
//! the overlays to flatten out the deltas, so there is no additional lookup walk (e.g., after the
//! completion of a transaction's execution in a block), and the management is clear and
//! predictable.
//!
//! # A Realistic Data Flow Example
//! To finish up the discussion about the data flow, we give a concrete design here to demonstrate
//! how versatile this abstraction could be, in an ASCII diagram:
//!
//! ```notrust
//!                             [ Core (TxExecState) ]
//!                                 |           ^ (Read)
//!                                 |           |
//!                           <WorldStateW> <WorldStateR>
//!                                 |           |
//!                                 v (Write)   |         _
//!             .------------[      MemStateAuto       ]   `.
//!             |            [      MemStateAuto       ]    | versioning
//!             |            [      MemStateAuto       ]    |
//!             |        .---[      MemStateAuto       ]    |
//!             |        |   [      MemStateAuto       ]  _/
//!       consolidate()  v                        ^
//!             |       ... (commit(root0))       |
//!             |                                 |
//!             v                                 |
//!        [MemStateShared]                       |
//!             |                                 |
//!       consolidate_delta()               <WorldStateR>
//!             v                                 |
//!         [StateDelta]                          |
//!             |                          (external source)
//!             |------------(update)------------ |
//!             |                               | |
//!             v                               v |
//!          commit(root1) <---<AccountRootR>--[Some Cache]
//!             |                                 |
//!             |                                 |
//!             v                                 |
//!     [Persistent Store] == read() ==> <WorldStateR object>
//! ```
//!
//! Note: [AccountRootR](state/trait.AccountRootR.html) is a trait that provides account state root
//! query in addition to `WorldStateR`. For a system without a cache layer (`[Some Cache]`), the
//! persistent store can implement such `AccountRootR` object that can be used both in `commit()`
//! and `read()` so reads during the commit phase will be directly from the persistent store.

#[macro_use] extern crate num_derive;

#[macro_use]
pub mod common;
#[cfg(feature = "chain")] pub mod chain;
pub mod core;
#[cfg(feature = "actor")] pub mod mempool;
pub mod processor;
#[cfg(feature = "rpc")] pub mod rpc;
pub mod state;
#[cfg(feature = "tx")] pub mod tx;
