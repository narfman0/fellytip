# The Sunken Realm — Lore & History

## The Civilization: The Kindled (Auremn)

They named themselves *Auremn* — "those who woke the world" — which reveals everything about their hubris. They had no active gods. Where other civilizations prayed for fire, the Kindled *built* fire.

Necessity produced their technology: a discipline called **resonant thaumaturgy** — the discovery that ordered magical resonance could locally lower entropy, turning the world's thermal gradients into computational and industrial power. Crystalline lattices stored spells the way stone stores weight. Arcane engineering was simply engineering.

They weren't evil. They were *confident*. They assumed a cosmos without intervening gods was also an unpoliced one. They were wrong.

## The Entity: The Unmaking

When the Kindled pushed their resonance deep enough into the crust, they struck the Weave — the substrate on which both physics and magic compute. And they found a wound.

The Unmaking is not a creature. It has no will, no hatred, no desire. It is an **acausal attractor**: a place where the Weave fell into a self-referential loop, and cause-and-effect became negotiable. What makes it terrifying is what it does: it dissolves distinction. A and not-A become the same. Fire and cold become the same. Memory and forgetting become the same.

A magic-tech civilization runs on categorization — *this is a circuit, this is a soul, this is fire.* The Unmaking doesn't destroy that. It *undifferentiates* it. Generic evil wants to rule; the Unmaking has no wants, because wanting requires a self distinct from its object.

## The Sacrifice: The Sealing

The leadership of the Kindled knew. The populace did not.

Their solution was a **topological inversion**: a ritual that folded the city's spatial neighborhood into a pocket beneath the crust. The city didn't fall underground. The city *became the lock*.

Every citizen who went under became load-bearing architecture in a seal that still runs. They aren't dead. They're holding.

This is the moral fracture that makes the myth powerful: the heroes didn't sacrifice themselves — they sacrificed *everyone*, without consent. The question (saviors or tyrants?) has no answer, which is why it persists in fragmented form across surface cultures.

## The Forgetting

It wasn't a cover-up. The Unmaking's nature leaked into memory itself.

Surface cultures don't suppress the Kindled — they *cannot hold the shape of them*. Folk songs have wrong endings. Children draw the old symbols without being taught. Words exist in multiple surface languages with no known etymology. The surface world is threaded with survivals nobody recognizes:

- Mountain passes that are too geometric (the old road network, naturalized)
- Rivers that bend wrong (the old canal system)
- "Star-iron" — mundane Kindled alloys plowed up by farmers, sold as meteorite metal
- Hereditary conditions in certain bloodlines that are actually magical resonance residue
- Children's counting rhymes that preserve fragments of Kindled safety protocols

The Sunken Realm is what surface people call it, when they call it anything. Most treat it as allegory. A few scholarly traditions take it seriously. State authorities don't know whether it falls under religion, history, or military affairs.

## The Underground Zones (in-game)

The fold is preserved but warped. Gravity points inward toward the old civic center. Sightlines curve. The deeper one goes, the more the fold's stitching shows — redundant geometry, rooms that repeat, rain that falls sideways.

### Three kinds of things that emerge

**Shards** — crystallized fragments of the Unmaking. Each is a local rule-rewriter with a narrow domain: a shard that makes fire cold, a shard that causes memory failure in a 10-foot radius, a shard that inverts gravity locally. They drift toward portal nodes.

**Revenants** — preserved Kindled citizens and automata, drifting outward when nodes open. Centuries out of time. They speak a language no surface scholar recognizes. Some are hostile. Some are simply confused. They know things.

**Fold-born** — creatures that evolved inside the warped pocket over centuries of isolation, adapted to negotiable physics. Genuinely alien. No known taxonomy.

## The Portal Nodes (Chaos Portals)

The seal is a topological defect. It cannot be removed by any local process — only a matching anti-defect could cancel it, and creating one would require repeating the original catastrophe.

The seal leaks at points of minimum curvature called **portal nodes**, which drift slowly. Players can:

- **Pin** a node — stabilize it, prevent further drift, at a cost
- **Shift** a node — redirect the leak elsewhere; trade one problem for another  
- **Widen** a node — catastrophic and rarely wise, but sometimes necessary

The seal requires maintenance. Which means it requires *people*.

## In-code naming

| Lore term | Code identifier |
|-----------|----------------|
| The Sunken Realm | `ZoneKind::Underground { depth }` |
| Portal node / chaos portal | `PortalKind::SealRift` |
| Threat signal | `StoryEventKind::UndergroundThreat` |
| Pressure resource | `UndergroundPressure` |
| The Kindled / Remnants (faction) | `faction_id: "remnants"` |
