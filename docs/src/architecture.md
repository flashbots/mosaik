# Architecture Overview

This page presents the Mosaik platform from the highest concepts down to the core primitives that power each actor and protocol feature.

## Platform Topology
Mosaik orchestrates a distributed order-pool fabric that sits between external order producers and the downstream sequencer. Wallets speak to the RPC front end, which feeds pending-state logic, while the builder and Engine API nodes exchange canonical payloads with execution clients.

<div class="mermaid">
flowchart TD
    Wallets[[External Wallets]] --> RPC["RPC Frontend"]
    RPC --> Pending["Pending Pool"]
    Pending --> Builder["Builder Node"]
    Builder --> EngineAPI["Engine API"]
    EngineAPI --> Sequencer[[Sequencer / Execution Client]]
    Builder --> Canon["Canon State"]
    Canon --> RPC
    Canon --> GC["Garbage Collector"]
</div>

Key code: [actors](https://github.com/flashbots/mosaik/tree/main/actors), [actors/rpc](https://github.com/flashbots/mosaik/tree/main/actors/rpc), [actors/engine-api](https://github.com/flashbots/mosaik/tree/main/actors/engine-api).

## Actor Layer
Each operational concern runs as its own binary crate under `actors/`, enabling independent scaling while sharing the same library.

<div class="mermaid">
graph LR
    PendingActor["actors/pending"] --> BuilderActor["actors/builder"]
    BuilderActor --> EngineLink["actors/engine-api"]
    PendingActor --> RPCActor["actors/rpc"]
    CanonActor["actors/canon"] --> RPCActor
    CanonActor --> GCActor["actors/gc"]
    ReputationActor["actors/reputation"] --> PendingActor
    Simulator["actors/sim"] --> PendingActor
</div>

Reference code: [actors/pending](https://github.com/flashbots/mosaik/tree/main/actors/pending), [actors/builder](https://github.com/flashbots/mosaik/tree/main/actors/builder), [actors/canon](https://github.com/flashbots/mosaik/tree/main/actors/canon), [actors/gc](https://github.com/flashbots/mosaik/tree/main/actors/gc), [actors/reputation](https://github.com/flashbots/mosaik/tree/main/actors/reputation), [actors/sim](https://github.com/flashbots/mosaik/tree/main/actors/sim).

## Core Library Modules
The shared library in `src/` offers the deterministic logic required by every actor: networking, stream orchestration, membership, and discovery services.

<div class="mermaid">
graph TD
    Core["src/lib.rs"] --> Network["src/network.rs"]
    Core --> Streams["src/streams"]
    Core --> Discovery["src/discovery"]
    Core --> Groups["src/groups"]
    Core --> Local["src/local.rs"]
    Network --> Interests["src/discovery/interests.rs"]
    Streams --> Producers["src/streams/producers.rs"]
    Streams --> Consumers["src/streams/consumers.rs"]
</div>

Relevant files: [src/lib.rs](https://github.com/flashbots/mosaik/blob/main/src/lib.rs), [src/network.rs](https://github.com/flashbots/mosaik/blob/main/src/network.rs), [src/streams](https://github.com/flashbots/mosaik/tree/main/src/streams), [src/discovery](https://github.com/flashbots/mosaik/tree/main/src/discovery), [src/groups](https://github.com/flashbots/mosaik/tree/main/src/groups), [src/local.rs](https://github.com/flashbots/mosaik/blob/main/src/local.rs).

## Low-Level Primitives & Utilities
Identifiers, serialization helpers, and shared test utilities live alongside the core modules to keep protocol logic deterministic and testable.

<div class="mermaid">
graph LR
    ID["src/id.rs"] --> Network
    ID --> Streams
    Serialization["src/local.rs"] --> Streams
    TestUtils["src/test_utils.rs"] -- "used by" --> Tests["tests/*"]
</div>

See: [src/id.rs](https://github.com/flashbots/mosaik/blob/main/src/id.rs), [src/local.rs](https://github.com/flashbots/mosaik/blob/main/src/local.rs), [src/test_utils.rs](https://github.com/flashbots/mosaik/blob/main/src/test_utils.rs), [tests](https://github.com/flashbots/mosaik/tree/main/tests).
