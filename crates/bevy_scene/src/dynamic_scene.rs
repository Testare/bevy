use crate::{DynamicSceneBuilder, Scene, SceneSpawnError};
use anyhow::Result;
use bevy_app::AppTypeRegistry;
use bevy_ecs::{
    entity::EntityMap,
    reflect::{ReflectComponent, ReflectMapEntities},
    world::World,
};
use bevy_reflect::{Reflect, TypeRegistryArc, TypeUuid};
use bevy_utils::HashMap;

#[cfg(feature = "serialize")]
use crate::serde::SceneSerializer;
use bevy_ecs::reflect::ReflectResource;
#[cfg(feature = "serialize")]
use serde::Serialize;

/// A collection of serializable resources and dynamic entities.
///
/// Each dynamic entity in the collection contains its own run-time defined set of components.
/// To spawn a dynamic scene, you can use either:
/// * [`SceneSpawner::spawn_dynamic`](crate::SceneSpawner::spawn_dynamic)
/// * adding the [`DynamicSceneBundle`](crate::DynamicSceneBundle) to an entity
/// * adding the [`Handle<DynamicScene>`](bevy_asset::Handle) to an entity (the scene will only be
/// visible if the entity already has [`Transform`](bevy_transform::components::Transform) and
/// [`GlobalTransform`](bevy_transform::components::GlobalTransform) components)
#[derive(Default, TypeUuid)]
#[uuid = "749479b1-fb8c-4ff8-a775-623aa76014f5"]
pub struct DynamicScene {
    pub resources: Vec<Box<dyn Reflect>>,
    pub entities: Vec<DynamicEntity>,
}

/// A reflection-powered serializable representation of an entity and its components.
pub struct DynamicEntity {
    /// The transiently unique identifier of a corresponding `Entity`.
    pub entity: u32,
    /// A vector of boxed components that belong to the given entity and
    /// implement the `Reflect` trait.
    pub components: Vec<Box<dyn Reflect>>,
}

impl DynamicScene {
    /// Create a new dynamic scene from a given scene.
    pub fn from_scene(scene: &Scene, type_registry: &AppTypeRegistry) -> Self {
        Self::from_world(&scene.world, type_registry)
    }

    /// Create a new dynamic scene from a given world.
    pub fn from_world(world: &World, type_registry: &AppTypeRegistry) -> Self {
        let mut builder =
            DynamicSceneBuilder::from_world_with_type_registry(world, type_registry.clone());

        builder.extract_entities(world.iter_entities().map(|entity| entity.id()));
        builder.extract_resources();

        builder.build()
    }

    /// Write the resources, the dynamic entities, and their corresponding components to the given world.
    ///
    /// This method will return a [`SceneSpawnError`] if a type either is not registered
    /// in the provided [`AppTypeRegistry`] resource, or doesn't reflect the
    /// [`Component`](bevy_ecs::component::Component) or [`Resource`](bevy_ecs::prelude::Resource) trait.
    pub fn write_to_world_with(
        &self,
        world: &mut World,
        entity_map: &mut EntityMap,
        type_registry: &AppTypeRegistry,
    ) -> Result<(), SceneSpawnError> {
        let type_registry = type_registry.read();

        for resource in &self.resources {
            let registration = type_registry
                .get_with_name(resource.type_name())
                .ok_or_else(|| SceneSpawnError::UnregisteredType {
                    type_name: resource.type_name().to_string(),
                })?;
            let reflect_resource = registration.data::<ReflectResource>().ok_or_else(|| {
                SceneSpawnError::UnregisteredResource {
                    type_name: resource.type_name().to_string(),
                }
            })?;

            // If the world already contains an instance of the given resource
            // just apply the (possibly) new value, otherwise insert the resource
            reflect_resource.apply_or_insert(world, &**resource);
        }

        // Collection of entities that have references to entities in the scene,
        // that need to be updated to entities in the world.
        // Keyed by Component's TypeId.
        let mut entity_mapped_entities = HashMap::default();

        for scene_entity in &self.entities {
            // Fetch the entity with the given entity id from the `entity_map`
            // or spawn a new entity with a transiently unique id if there is
            // no corresponding entry.
            let entity = *entity_map
                .entry(bevy_ecs::entity::Entity::from_raw(scene_entity.entity))
                .or_insert_with(|| world.spawn_empty().id());
            let entity_mut = &mut world.entity_mut(entity);

            // Apply/ add each component to the given entity.
            for component in &scene_entity.components {
                let registration = type_registry
                    .get_with_name(component.type_name())
                    .ok_or_else(|| SceneSpawnError::UnregisteredType {
                        type_name: component.type_name().to_string(),
                    })?;
                let reflect_component =
                    registration.data::<ReflectComponent>().ok_or_else(|| {
                        SceneSpawnError::UnregisteredComponent {
                            type_name: component.type_name().to_string(),
                        }
                    })?;

                // If this component references entities in the scene, track it
                // so we can update it to the entity in the world.
                if registration.data::<ReflectMapEntities>().is_some() {
                    entity_mapped_entities
                        .entry(registration.type_id())
                        .or_insert(Vec::new())
                        .push(entity);
                }

                // If the entity already has the given component attached,
                // just apply the (possibly) new value, otherwise add the
                // component to the entity.
                reflect_component.apply_or_insert(entity_mut, &**component);
            }
        }

        // Updates references to entities in the scene to entities in the world
        for (type_id, entities) in entity_mapped_entities.into_iter() {
            let registration = type_registry.get(type_id).unwrap();
            if let Some(map_entities_reflect) = registration.data::<ReflectMapEntities>() {
                map_entities_reflect
                    .map_specific_entities(world, entity_map, entities)
                    .unwrap();
            }
        }

        Ok(())
    }

    /// Write the resources, the dynamic entities, and their corresponding components to the given world.
    ///
    /// This method will return a [`SceneSpawnError`] if a type either is not registered
    /// in the world's [`AppTypeRegistry`] resource, or doesn't reflect the
    /// [`Component`](bevy_ecs::component::Component) trait.
    pub fn write_to_world(
        &self,
        world: &mut World,
        entity_map: &mut EntityMap,
    ) -> Result<(), SceneSpawnError> {
        let registry = world.resource::<AppTypeRegistry>().clone();
        self.write_to_world_with(world, entity_map, &registry)
    }

    // TODO: move to AssetSaver when it is implemented
    /// Serialize this dynamic scene into rust object notation (ron).
    #[cfg(feature = "serialize")]
    pub fn serialize_ron(&self, registry: &TypeRegistryArc) -> Result<String, ron::Error> {
        serialize_ron(SceneSerializer::new(self, registry))
    }
}

/// Serialize a given Rust data structure into rust object notation (ron).
#[cfg(feature = "serialize")]
pub fn serialize_ron<S>(serialize: S) -> Result<String, ron::Error>
where
    S: Serialize,
{
    let pretty_config = ron::ser::PrettyConfig::default()
        .indentor("  ".to_string())
        .new_line("\n".to_string());
    ron::ser::to_string_pretty(&serialize, pretty_config)
}

#[cfg(test)]
mod tests {
    use bevy_app::AppTypeRegistry;
    use bevy_ecs::{entity::EntityMap, system::Command, world::World};
    use bevy_hierarchy::{AddChild, Parent};

    use crate::dynamic_scene_builder::DynamicSceneBuilder;

    #[test]
    fn components_not_defined_in_scene_should_not_be_effected_by_scene_entity_map() {
        // Testing that scene reloading applies EntitiyMap correctly to MapEntities components.

        // First, we create a simple world with a parent and a child relationship
        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        world
            .resource_mut::<AppTypeRegistry>()
            .write()
            .register::<Parent>();
        let gen_0_entity = world.spawn_empty().id();
        let gen_1_entity = world.spawn_empty().id();
        AddChild {
            parent: gen_0_entity,
            child: gen_1_entity,
        }
        .write(&mut world);

        // We then write this relationship to a new scene, and then write that scene back to the world to create another parent and child relationship
        let mut scene_builder = DynamicSceneBuilder::from_world(&world);
        scene_builder.extract_entity(gen_0_entity);
        scene_builder.extract_entity(gen_1_entity);
        let scene = scene_builder.build();
        let mut entity_map = EntityMap::default();
        scene.write_to_world(&mut world, &mut entity_map).unwrap();

        // We then add the parent in the scene relationship (gen_2) as a child of the first child relationship (gen_1)
        let gen_2_entity = entity_map.get(gen_0_entity).unwrap();
        let gen_3_entity = entity_map.get(gen_1_entity).unwrap();
        AddChild {
            parent: gen_1_entity,
            child: gen_2_entity,
        }
        .write(&mut world);

        // We then reload the scene to make sure that gen_2_entity's parent component isn't updated with the entity map, since this component isn't defined in the scene.
        // With bevy_hierarchy, this can cause serious errors and malformed hierarchies.
        scene.write_to_world(&mut world, &mut entity_map).unwrap();

        assert_eq!(
            gen_0_entity,
            world
                .get_entity(gen_1_entity)
                .unwrap()
                .get::<Parent>()
                .unwrap()
                .get(),
            "Something about reloading the scene is touching entities with the same scene Ids"
        );
        assert_eq!(
            gen_1_entity,
            world
                .get_entity(gen_2_entity)
                .unwrap()
                .get::<Parent>()
                .unwrap()
                .get(),
            "Something about reloading the scene is touching components not defined in the scene but on entities defined in the scene"
        );
        assert_eq!(
            gen_2_entity,
            world
                .get_entity(gen_3_entity)
                .unwrap()
                .get::<Parent>()
                .expect("Something is wrong with this test, and the scene components don't have a parent/child relationship")
                .get(),
            "Something is wrong with the this test or the code reloading scenes since the relationship between scene entities is broken"
        );
    }
}
