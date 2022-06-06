use crate::{components::*, resources::*, utils::*, Contact, DELTA_TIME, SUB_DT};

use bevy::prelude::*;

/// Collects bodies that are potentially colliding and stores them in pairs in the [`CollisionPairs`] array.
pub(crate) fn collect_collision_pairs(
    query: Query<(Entity, &Pos, &Rot, &LinVel, &Collider)>,
    mut collision_pairs: ResMut<CollisionPairs>,
) {
    collision_pairs.0.clear();

    // Safety margin multiplier bigger than DELTA_TIME to account for sudden accelerations
    let safety_margin_factor = 2.0 * DELTA_TIME;

    let mut iter = query.iter_combinations();
    while let Some(
        [(ent_a, pos_a, rot_a, vel_a, collider_a), (ent_b, pos_b, rot_b, vel_b, collider_b)],
    ) = iter.fetch_next()
    {
        let aabb_a = aabb_with_margin(
            &pos_a.0,
            rot_a,
            &collider_a.shape,
            safety_margin_factor * vel_a.length(),
        );

        let aabb_b = aabb_with_margin(
            &pos_b.0,
            rot_b,
            &collider_b.shape,
            safety_margin_factor * vel_b.length(),
        );

        if aabb_a.intersection(&aabb_b).is_some() {
            collision_pairs.0.push((ent_a, ent_b));
        }
    }
}

/// Applies forces and predicts the next position and velocity for all dynamic bodies.
pub(crate) fn integrate_pos(
    mut query: Query<(
        &mut Pos,
        &mut PrevPos,
        &mut LinVel,
        &mut PreSolveLinVel,
        &Density,
    )>,
    gravity: Res<Gravity>,
) {
    for (mut pos, mut prev_pos, mut vel, mut pre_solve_vel, mass) in query.iter_mut() {
        prev_pos.0 = pos.0;

        let gravitation_force = mass.0 * gravity.0;
        let external_forces = gravitation_force;
        vel.0 += SUB_DT * external_forces / mass.0;
        pos.0 += SUB_DT * vel.0;
        pre_solve_vel.0 = vel.0;
    }
}

/// Integrates rotations for all dynamic bodies.
pub(crate) fn integrate_rot(
    mut query: Query<(&mut Rot, &mut PrevRot, &AngVel, &mut PreSolveAngVel)>,
) {
    for (mut rot, mut prev_rot, ang_vel, mut pre_solve_ang_vel) in query.iter_mut() {
        prev_rot.cos = rot.cos;
        prev_rot.sin = rot.sin;
        *rot += Rot::from_radians(SUB_DT * ang_vel.0);
        pre_solve_ang_vel.0 = ang_vel.0;
    }
}

/// Solves position constraints for dynamic-dynamic interactions.
pub(crate) fn solve_pos_dynamics(
    mut query: Query<(
        &mut Pos,
        &mut Rot,
        &PrevPos,
        &Friction,
        &Collider,
        &MassProperties,
    )>,
    collision_pairs: Res<CollisionPairs>,
    mut contacts: ResMut<DynamicContacts>,
) {
    for (ent_a, ent_b) in collision_pairs.0.iter() {
        if let Ok(
            [(mut pos_a, mut rot_a, prev_pos_a, friction_a, collider_a, mass_props_a), (mut pos_b, mut rot_b, prev_pos_b, friction_b, collider_b, mass_props_b)],
        ) = query.get_many_mut([*ent_a, *ent_b])
        {
            if let Some(contact) = get_contact(
                *ent_a,
                *ent_b,
                pos_a.0,
                pos_b.0,
                rot_a.as_radians(),
                rot_b.as_radians(),
                &collider_a.shape,
                &collider_b.shape,
            ) {
                constrain_positions_dynamic_dynamic(
                    &mut pos_a,
                    &mut pos_b,
                    &mut rot_a,
                    &mut rot_b,
                    prev_pos_a,
                    prev_pos_b,
                    friction_a,
                    friction_b,
                    mass_props_a,
                    mass_props_b,
                    contact,
                );

                contacts.0.push(contact);
            }
        }
    }
}

/// Solves position constraints for dynamic-static interactions.
pub(crate) fn solve_pos_statics(
    mut dynamics: Query<(
        Entity,
        &mut Pos,
        &mut Rot,
        &PrevPos,
        &Friction,
        &Collider,
        &MassProperties,
    )>,
    statics: Query<(Entity, &Pos, &Rot, &Friction, &Collider), Without<MassProperties>>,
    mut contacts: ResMut<StaticContacts>,
) {
    for (ent_a, mut pos_a, mut rot_a, prev_pos_a, friction_a, collider_a, mass_props) in
        dynamics.iter_mut()
    {
        for (ent_b, pos_b, rot_b, friction_b, collider_b) in statics.iter() {
            if let Some(contact) = get_contact(
                ent_a,
                ent_b,
                pos_a.0,
                pos_b.0,
                rot_a.as_radians(),
                rot_b.as_radians(),
                &collider_a.shape,
                &collider_b.shape,
            ) {
                constrain_position_dynamic_static(
                    &mut pos_a, &mut rot_a, prev_pos_a, friction_a, friction_b, mass_props, contact,
                );

                contacts.0.push(contact);
            }
        }
    }
}

/// Solves overlap between two dynamic bodies according to their masses.
#[allow(clippy::too_many_arguments)]
fn constrain_positions_dynamic_dynamic(
    pos_a: &mut Pos,
    pos_b: &mut Pos,
    rot_a: &mut Rot,
    rot_b: &mut Rot,
    prev_pos_a: &PrevPos,
    prev_pos_b: &PrevPos,
    friction_a: &Friction,
    friction_b: &Friction,
    mass_props_a: &MassProperties,
    mass_props_b: &MassProperties,
    contact: Contact,
) {
    let Contact {
        normal,
        penetration,
        r_a,
        r_b,
        ..
    } = contact;

    let w_a_rot = mass_props_a.inv_inertia * r_a.perp_dot(normal).powi(2);
    let w_b_rot = mass_props_b.inv_inertia * r_b.perp_dot(normal).powi(2);

    let w_a = mass_props_a.inv_mass + w_a_rot;
    let w_b = mass_props_b.inv_mass + w_b_rot;

    let w = w_a + w_b;
    let p = -normal * penetration / w;

    pos_a.0 += p * w_a;
    pos_b.0 -= p * w_b;

    *rot_a += Rot::from_radians(mass_props_a.inv_inertia * r_a.perp_dot(p));
    *rot_b += Rot::from_radians(mass_props_b.inv_inertia * r_b.perp_dot(-p));

    let friction = get_static_friction(
        pos_a.0 - prev_pos_a.0,
        pos_b.0 - prev_pos_b.0,
        friction_a,
        friction_b,
        normal,
        penetration / w,
    );
    pos_a.0 -= friction;
    pos_b.0 += friction;
}

/// Solves overlap between a dynamic body and a static body.
fn constrain_position_dynamic_static(
    pos: &mut Pos,
    rot: &mut Rot,
    prev_pos: &PrevPos,
    friction_a: &Friction,
    friction_b: &Friction,
    mass_props: &MassProperties,
    contact: Contact,
) {
    let Contact {
        normal,
        penetration,
        r_a: r,
        ..
    } = contact;

    let w_rot = mass_props.inv_inertia * r.perp_dot(normal).powi(2);
    let w = mass_props.inv_mass + w_rot;
    let p = -normal * penetration / w;

    pos.0 += p * mass_props.inv_mass;
    *rot += Rot::from_radians(mass_props.inv_inertia * r.perp_dot(p));

    pos.0 -= get_static_friction(
        pos.0 - prev_pos.0,
        Vec2::ZERO,
        friction_a,
        friction_b,
        normal,
        penetration / w,
    );
}

/// Updates the linear velocity of all dynamic bodies based on the change in position from the previous step.
pub(crate) fn update_lin_vel(mut query: Query<(&Pos, &PrevPos, &mut LinVel)>) {
    for (pos, prev_pos, mut vel) in query.iter_mut() {
        vel.0 = (pos.0 - prev_pos.0) / SUB_DT;
    }
}

/// Updates the angular velocity of all dynamic bodies based on the change in rotation from the previous step.
pub(crate) fn update_ang_vel(mut query: Query<(&Rot, &PrevRot, &mut AngVel)>) {
    for (rot, prev_rot, mut ang_vel) in query.iter_mut() {
        ang_vel.0 = (prev_rot.inv().mul(*rot)).as_radians() / SUB_DT;
    }
}

/// Solves the new velocities of dynamic bodies after dynamic-dynamic collisions.
#[allow(clippy::type_complexity)]
pub(crate) fn solve_vel_dynamics(
    mut query: Query<(
        &mut LinVel,
        &mut AngVel,
        &PreSolveLinVel,
        &PreSolveAngVel,
        &Restitution,
        &Friction,
        &Density,
        &Collider,
    )>,
    contacts: Res<DynamicContacts>,
) {
    for Contact {
        entity_a,
        entity_b,
        r_a,
        r_b,
        normal,
        penetration,
        ..
    } in contacts.0.iter().cloned()
    {
        if let Ok(
            [(
                mut lin_vel_a,
                mut ang_vel_a,
                pre_solve_lin_vel_a,
                pre_solve_ang_vel_a,
                restitution_a,
                friction_a,
                density_a,
                collider_a,
            ), (
                mut lin_vel_b,
                mut ang_vel_b,
                pre_solve_lin_vel_b,
                pre_solve_ang_vel_b,
                restitution_b,
                friction_b,
                density_b,
                collider_b,
            )],
        ) = query.get_many_mut([entity_a, entity_b])
        {
            let MassProperties {
                mass: mass_a,
                inv_mass: inv_mass_a,
                inv_inertia: inv_inertia_a,
                ..
            } = collider_a.mass_properties(density_a.0);
            let MassProperties {
                mass: mass_b,
                inv_mass: inv_mass_b,
                inv_inertia: inv_inertia_b,
                ..
            } = collider_b.mass_properties(density_b.0);

            let pre_solve_contact_vel_a =
                pre_solve_lin_vel_a.0 + pre_solve_ang_vel_a.0 * r_a.perp();
            let pre_solve_contact_vel_b =
                pre_solve_lin_vel_b.0 + pre_solve_ang_vel_b.0 * r_b.perp();
            let pre_solve_relative_vel = pre_solve_contact_vel_a - pre_solve_contact_vel_b;
            let pre_solve_normal_vel = normal.dot(pre_solve_relative_vel);

            let contact_vel_a = lin_vel_a.0 + ang_vel_a.0 * r_a.perp();
            let contact_vel_b = lin_vel_b.0 + ang_vel_b.0 * r_b.perp();
            let relative_vel = contact_vel_a - contact_vel_b;
            let normal_vel = normal.dot(relative_vel);

            let w_rot_a = inv_inertia_a * r_a.perp_dot(normal).powi(2);
            let w_rot_b = inv_inertia_b * r_b.perp_dot(normal).powi(2);

            let w_a = inv_mass_a + w_rot_a;
            let w_b = inv_mass_b + w_rot_b;

            let w = w_a + w_b;

            let tangent_vel = relative_vel - normal * normal_vel;
            let friction_force =
                get_dynamic_friction(tangent_vel, penetration, friction_a, friction_b);

            let restitution_force = get_restitution(
                normal,
                normal_vel,
                pre_solve_normal_vel,
                restitution_a,
                restitution_b,
            );

            // Apply velocity changes

            let p = (restitution_force - friction_force) / w;

            lin_vel_a.0 += p / mass_a;
            lin_vel_b.0 -= p / mass_b;

            ang_vel_a.0 += inv_inertia_a * r_a.perp_dot(p);
            ang_vel_b.0 -= inv_inertia_b * r_b.perp_dot(p);
        }
    }
}

/// Solves the new velocities of dynamic bodies after dynamic-static collisions.
#[allow(clippy::type_complexity)]
pub(crate) fn solve_vel_statics(
    mut dynamics: Query<(
        &mut LinVel,
        &mut AngVel,
        &PreSolveLinVel,
        &PreSolveAngVel,
        &Restitution,
        &Friction,
        &Density,
        &Collider,
    )>,
    statics: Query<(&Restitution, &Friction), Without<Density>>,
    contacts: Res<StaticContacts>,
) {
    for Contact {
        entity_a,
        entity_b,
        r_a: r,
        normal,
        penetration,
        ..
    } in contacts.0.iter().cloned()
    {
        if let Ok((
            mut lin_vel,
            mut ang_vel,
            pre_solve_lin_vel,
            pre_solve_ang_vel,
            restitution_a,
            friction_a,
            density,
            collider,
        )) = dynamics.get_mut(entity_a)
        {
            if let Ok((restitution_b, friction_b)) = statics.get(entity_b) {
                let MassProperties {
                    mass,
                    inv_mass,
                    inv_inertia,
                    ..
                } = collider.mass_properties(density.0);

                let pre_solve_contact_vel = pre_solve_lin_vel.0 + pre_solve_ang_vel.0 * r.perp();
                let pre_solve_normal_vel = Vec2::dot(pre_solve_contact_vel, normal);

                let contact_vel = lin_vel.0 + ang_vel.0 * r.perp();
                let normal_vel = Vec2::dot(contact_vel, normal);

                let w_rot = inv_inertia * r.perp_dot(normal).powi(2);

                let w = inv_mass + w_rot;

                let tangent_vel = contact_vel - normal * normal_vel;
                let friction_force =
                    get_dynamic_friction(tangent_vel, penetration, friction_a, friction_b);

                let restitution_force = get_restitution(
                    normal,
                    normal_vel,
                    pre_solve_normal_vel,
                    restitution_a,
                    restitution_b,
                );

                // Apply velocity changes

                let p = (restitution_force - friction_force) / w;

                lin_vel.0 += p / mass;
                ang_vel.0 += inv_inertia * r.perp_dot(p);
            }
        }
    }
}

/// Clears all contact resources.
pub(crate) fn clear_contacts(
    mut contacts: ResMut<DynamicContacts>,
    mut static_contacts: ResMut<StaticContacts>,
) {
    contacts.0.clear();
    static_contacts.0.clear();
}

/// Copies positions from the physics world to bevy Transforms
pub(crate) fn sync_transforms(mut query: Query<(&mut Transform, &Pos, &Rot)>) {
    for (mut transform, pos, rot) in query.iter_mut() {
        transform.translation = pos.0.extend(0.0);
        transform.rotation = (*rot).into();
    }
}

type MassPropsChanged = Or<(Changed<Density>, Changed<Collider>)>;

pub(crate) fn update_mass_props(
    mut query: Query<(&mut MassProperties, &Density, &Collider), MassPropsChanged>,
) {
    for (mut mass_props, density, collider) in query.iter_mut() {
        *mass_props = collider.mass_properties(density.0);
    }
}
