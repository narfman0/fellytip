//! Party system: groups up to 4 clients into a party, tracks membership,
//! and exposes a `PartyRegistry` resource for visibility culling and HUD sync.

use bevy::prelude::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;

/// Maximum clients per party (hard limit for Milestone 3).
// Used in tests and enforced in join(); visibility culling uses it in Step 13.
#[allow(dead_code)]
pub const MAX_PARTY_SIZE: usize = 4;

/// Stable party identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PartyId(pub u32);

/// Resource: all live parties on the server.
#[derive(Resource, Default)]
pub struct PartyRegistry {
    pub parties: Vec<Party>,
    #[allow(dead_code)]
    next_id: u32,
}

#[allow(dead_code)]
impl PartyRegistry {
    /// Create a new party with a single founding member. Returns the PartyId.
    pub fn create(&mut self, founder: Entity) -> PartyId {
        let id = PartyId(self.next_id);
        self.next_id += 1;
        self.parties.push(Party {
            id: id.clone(),
            members: vec![founder],
        });
        tracing::info!("Party {:?} created by {:?}", id.0, founder);
        id
    }

    /// Add a client to an existing party. Returns false if full or not found.
    pub fn join(&mut self, party_id: &PartyId, member: Entity) -> bool {
        if let Some(party) = self.parties.iter_mut().find(|p| &p.id == party_id) {
            if party.members.len() >= MAX_PARTY_SIZE {
                tracing::warn!("Party {:?} is full", party_id.0);
                return false;
            }
            party.members.push(member);
            tracing::info!("Entity {:?} joined party {:?}", member, party_id.0);
            true
        } else {
            false
        }
    }

    /// Remove a member from all parties; disband empty parties.
    pub fn leave(&mut self, member: Entity) {
        self.parties.retain_mut(|party| {
            party.members.retain(|&m| m != member);
            !party.members.is_empty()
        });
    }
}

#[derive(Clone, Debug)]
pub struct Party {
    #[allow(dead_code)]
    pub id: PartyId,
    pub members: Vec<Entity>,
}

/// Server-only component: which party (if any) a client player entity belongs to.
#[derive(Component)]
#[allow(dead_code)]
pub struct PartyMember {
    #[allow(dead_code)]
    pub party_id: PartyId,
}

pub struct PartyPlugin;

impl Plugin for PartyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PartyRegistry>();
        app.add_observer(on_client_disconnected_party_cleanup);
    }
}

/// When a client disconnects, remove them from any party they were in.
fn on_client_disconnected_party_cleanup(
    trigger: On<Add, Disconnected>,
    query: Query<(), With<ClientOf>>,
    mut registry: ResMut<PartyRegistry>,
) {
    if query.get(trigger.entity).is_ok() {
        registry.leave(trigger.entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_entity(i: u32) -> Entity {
        // Construct a Entity with a stable index for testing.
        Entity::from_raw(i)
    }

    #[test]
    fn create_and_join() {
        let mut registry = PartyRegistry::default();
        let e0 = dummy_entity(0);
        let e1 = dummy_entity(1);
        let id = registry.create(e0);
        assert!(registry.join(&id, e1));
        let party = registry.parties.iter().find(|p| p.id == id).unwrap();
        assert_eq!(party.members.len(), 2);
    }

    #[test]
    fn party_capped_at_max() {
        let mut registry = PartyRegistry::default();
        let e0 = dummy_entity(0);
        let id = registry.create(e0);
        for i in 1..MAX_PARTY_SIZE {
            assert!(registry.join(&id, dummy_entity(i as u32)));
        }
        // One more should fail
        assert!(!registry.join(&id, dummy_entity(99)));
    }

    #[test]
    fn leave_disbands_empty_party() {
        let mut registry = PartyRegistry::default();
        let e0 = dummy_entity(0);
        let id = registry.create(e0);
        registry.leave(e0);
        assert!(registry.parties.iter().all(|p| p.id != id));
    }
}
