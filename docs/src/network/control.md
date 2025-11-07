# Control Network

All nodes on the network belong to a p2p gossip network that runs on top of [`iroh-gossip`](https://www.iroh.computer/proto/iroh-gossip).

## Peer Onboarding

During instantiation nodes specify the network id they want to join and a list of one or more peers that are already on the network:

```rust
let bootstrap = vec!["10.56.17.1:61092", "boot1.network.net"];
let network = Network::connect("network-id", bootstrap);
```

When bootstrap peers are contacted, the network discovery and definition protocol kicks in.

## Network Definition

Each node maintains a local copy of a `NetworkView` data structure. The network view structure is a key-value store that is populated by accumulating `PeerEntry` gossip messages from peers. Gossip messages have the following format:

```rust
  // local endpoint
  let me = Endpoint::bind().await?;

  // set once during node startup
  let update_id = UpdateId::default();

  // the protocol will reject any updates if this value is too distant from each peer's local time.
  let updated_at = Utc::now();

  // contains a list of all known addresses that this node is aware of that can be used to reach it.
  let addresses: endpoint.addr(); 

  let own_entry: SignedPeerEntry = PeerEntry {
    endpoint: me.addr(),
    update_id: update_id,
    updated_at: updated_at,
    interests: Interests::default(),
  }.sign(endpoint.secret_key());
```

Those messages are then propagated to the p2p gossip topic of the control network. Each peer on the control network upon receiving `SignedPeerEntry` messages will validate if the signature of the `PeerEntry` matches the id of the peer producing the message and if so, peers will store locally the `PeerEntry`. Local store of each peer is a key-value store, where keys are peer ids (public keys) and values are most recent values of `PeerEntry`. A `PeerEntry` overrides an existing entry for a peer iif the `UpdateId` is strictly greater than the local version.

The goal of the system is to ensure that each peer on the network eventually has the same consistent view of of the network. 

One way of achieving this is that whenever a node has a local change to the `NetworkView` structure, it will generate a merkle root for its `NetworkView` state and broadcast it to the gossip topic of the network. When peers receive a message with a `NetworkView` state root that is different than their own, they will know that they are out of sync with the broadcaster of the message. The aim will be to converge all peers eventually to the same network view state root.

`UpdateId` is a structure that is used to version `PeerEntry` updates. It has the following structure:

```rust
struct UpdateId {
  pub run_id: u64,
  pub seq: u64,
}
```

`run_id` is the timestamp of the process start by convention, so each new instance of the same process will have a larger `run_id`, which means that it has a higher incarnation number. `seq` is incremented monotonically by a running process with each update to its own `PeerEntry`.

Notes:

- Global network constants such as TTL are defined globally at the network level and in the initial iteration of the protocol they will be hardcoded in the code, so each peer does not need to specify TTL of its update.

- In stable long running network, the state root on all nodes should eventually stabilize and only change when individual machines leave or enter the network or if they update their interests.

- Changes to the network topology happen when nodes fail, during autoscaling events, sometimes when some services are reconfigured or added.

- Initially during network boot, when we are going from zero to hundred nodes changes to the network view will be very dynamic but should stabilize eventually.

- Each `PeerEntry` is leased for some TTL, and any entry that's `updated_at` value is older than a globally configured TTL will be dropped locally on all nodes and not propagated any more to other nodes, effectively deleting it from the network.

- When a new node joins the network the procedure is as following:
  1. It will sync locally the latest state of the current `NetworkView`
  2. Then it will broadcast through gossip its own signed `PeerEntry` along with the merkle root of its local `NetworkView` that already includes its own entry.
  3. Other nodes on the network will include the new `PeerEntry` locally and if the resulting merkle root is equal to the root broadcasted in step 2, then they know that they are up to date. If the state root differs then a full sync will occur.
  4. We need to account for cases (especially during network boot and aggressive autoscaling) when many nodes will go through step 2 simultaneously in bursts.

We need to design a protocol that will cover the following scenarios:

- Support network sizes of 1 to 1000 nodes.
- Support cold boot from zero nodes up to full topology.
- Keep all `NetworkView` instances in sync on all machines.
- React to changes to the network topology.

Let's first focus on iterating on the concepts and design level and when we are happy we will then move on to working on concrete code implementation.