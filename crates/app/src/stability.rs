//! Keeping a long visualizer run healthy. Reaction jets add fluid to particle solvers;
//! over a long run they'd fill the tank to the particle cap and stay there. Jets pause
//! while the particle count is over budget (splashes, which only push, carry on).

use sim::World;

/// Particles allowed above the starting count: a quarter more (or a fixed allowance
/// for scenes that start empty, like the wind tunnel).
pub fn particle_budget(initial: usize) -> usize {
    if initial == 0 { 20_000 } else { initial + initial / 4 }
}

/// Whether reactions may add fluid: always for grid solvers (their "fluid" is dye or
/// fuel, which can't pile up as particles), for particle solvers while under budget.
pub fn room_for_fluid(world: &World) -> bool {
    world.solver.particles().is_none_or(|p| p.pos.len() < particle_budget(world.initial_particles))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_allows_a_quarter_more_or_a_fixed_amount_from_empty() {
        assert_eq!(particle_budget(20_000), 25_000);
        assert_eq!(particle_budget(0), 20_000);
    }

    #[test]
    fn grid_solvers_always_have_room() {
        let world = World::new(2, 0); // stable fluids
        assert!(world.solver.particles().is_none() && room_for_fluid(&world));
        let world = World::new(0, 0); // FLIP, at its starting count
        assert!(room_for_fluid(&world));
    }
}
