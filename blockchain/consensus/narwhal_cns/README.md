# Narwhal Consensus

_Narwhal Consensus_ replaces most of the built-in Forest/Filecoin blockchain stack with an external consensus mechanism. It uses the DAG based [Narwhal](https://arxiv.org/pdf/2105.11827.pdf) to replace the mempool gossiping with Byzantine Atomic Broadcast of batched transactions, and [Bullshark](https://arxiv.org/abs/2201.05677) to establish a deterministic total ordering of the batches.

The initial integration notes can be seen [here](https://hackmd.io/pGpXHTTITl6iSLfmvb3KBw?view) and the options discussed [here](https://github.com/protocol/ConsensusLab/discussions/165).

We use the [MystenLabs Narwhal](https://github.com/MystenLabs/narwhal) implementation, which comes with its own networking stack. We derive deterministic Filecoin blocks from the ordered transaction batches, which requires that we completely disable block gossiping in Forest - otherwise we'd have to have agreement on who is eligible to create blocks, and we'd have to validate blocks we receive from the Filecoin pubsub. This would defeat the purpose of using external total ordering. Instead, signatures over the (hopefully) identical chains built in isolation will be shared and aggregated into checkpoints.
