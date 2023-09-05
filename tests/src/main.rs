//! Integration tests

use std::io::Write;

use anyhow::{bail, Result};
use fs_err::File;

use rand::seq::SliceRandom;

use abstio::{CityName, MapName};
use abstutil::Timer;
use blockfinding::Perimeter;
use geom::{Distance, Duration, Time};
use map_model::{IntersectionID, LaneType, Map, RoadID};
use sim::{AlertHandler, PrebakeSummary, Sim, SimFlags, SimOptions};
use synthpop::{IndividTrip, PersonSpec, Scenario, TripEndpoint, TripMode, TripPurpose};

use ::tests::{compare_with_goldenfile, import_map};

fn main() -> Result<()> {
    abstutil::logger::setup();
    if false {
        geometry_test()?;
    }
    test_blockfinding()?;
    test_lane_changing(&import_map(abstio::path(
        "../tests/input/lane_selection.osm",
    )))?;
    test_map_importer()?;
    check_proposals()?;
    if false {
        ab_test_spurious_diff()?;
    }
    if false {
        bus_test()?;
    }
    bus_route_test()?;
    if false {
        smoke_test()?;
    }
    Ok(())
}

/// Test the map pipeline by importing simple, handcrafted .osm files, then emitting goldenfiles
/// that summarize part of the generated map. Keep the goldenfiles under version control to notice
/// when they change. The goldenfiles (and changes to them) themselves aren't easy to understand,
/// but the test maps are.
fn test_map_importer() -> Result<()> {
    let regenerate_goldenfiles = false;

    for name in [
        "divided_highway_split",
        "left_turn_and_bike_lane",
        "multiple_left_turn_lanes",
        "false_positive_u_turns",
        "turn_restriction_ltn_boundary",
    ] {
        // TODO It's kind of a hack to reference the crate's directory relative to the data dir.
        let map = import_map(abstio::path(format!("../tests/input/{}.osm", name)));
        // Enable to debug the result with the normal GUI
        if false {
            map.save();
        }

        // Set this to true, when there is reason to regenerate the goldenfiles.
        // Should normally be false.
        if regenerate_goldenfiles {
            println!("Producing goldenfiles for {}", map.get_name().describe());
            dump_turn_goldenfile(&map)?;
            // Automatically fail when the goldenfiles are regenerated. This is so the test is not accidentally
            // left in a set where there goldenfiles are recreated on each run, and the test does not achieve its purpose.
            // assert!(false);
        }

        let path_types = abstio::path(format!(
            "../tests/goldenfiles/turn_types/{}.txt",
            map.get_name().map
        ));
        let turn_types = all_turn_info_as_string(&map);

        assert!(compare_with_goldenfile(turn_types, path_types).is_ok());
    }

    if regenerate_goldenfiles {
        // Automatically fail when the goldenfiles are regenerated. This is so the test is not accidentally
        // left in a set where there goldenfiles are recreated on each run, and the test does not achieve its purpose.
        panic!("Automatically fail when the goldenfiles are regenerated. This is so the test is not accidentally left in a set where there goldenfiles are recreated on each run, and the test does not achieve its purpose.")
    }
    Ok(())
}

/// Verify what turns are generated by writing (from lane, to lane, turn type).
fn dump_turn_goldenfile(map: &Map) -> Result<()> {
    let path_types = abstio::path(format!(
        "../tests/goldenfiles/turn_types/{}.txt",
        map.get_name().map
    ));
    let mut f_types = File::create(&path_types)?;
    let turn_types = all_turn_info_as_string(map);
    writeln!(f_types, "{}", turn_types)?;
    Ok(())
}

/// Generates a deterministic string, describing all of the turn type and the turn restrictions in a map.
/// The string is not easily human-readable, but can be used to detect changes is the map itself, or in the
/// code for importing the map and parsing the turn restrictions.
fn all_turn_info_as_string(map: &Map) -> String {
    let mut s = String::new();
    s.push_str("Turns:\n------\n");
    for t in map.all_turns() {
        s.push_str(&format!("{} is a {:?}\n", t.id, t.turn_type));
    }

    s.push_str("\n------------\nRestrictions:\n------------\n");
    for r1 in map.all_roads() {
        for (restriction, r2) in &r1.turn_restrictions {
            let (t_type, sign_pt, _, i_id) = map.get_ban_turn_info(r1, map.get_r(*r2));
            let i = map.get_i(i_id);
            s.push_str(&format!(
                "Turn from {} into {}, at intersection {:?} is a {:?}, type {:?}, location {}\n",
                r1.id, r2, i.id, restriction, t_type, sign_pt
            ));
        }

        for (via, r2) in &r1.complicated_turn_restrictions {
            s.push_str(&format!(
                "complicated turn: from {0:?}, via {1:?}, to {2:?}",
                (r1.orig_id.osm_way_id, r1.id),
                (map.get_r(*via).orig_id.osm_way_id, map.get_r(*via).id),
                (map.get_r(*r2).orig_id.osm_way_id, map.get_r(*r2).id)
            ));
        }
        // s.push_str("\n");
    }

    s
}

/// Simulate an hour on every map.
fn smoke_test() -> Result<()> {
    let mut timer = Timer::new("run a smoke-test for all maps");
    for name in MapName::list_all_maps_locally() {
        let map = map_model::Map::load_synchronously(name.path(), &mut timer);
        let scenario = if map.get_city_name() == &CityName::seattle() {
            abstio::read_binary(abstio::path_scenario(&name, "weekday"), &mut timer)
        } else {
            let mut rng = sim::SimFlags::for_test("smoke_test").make_rng();
            sim::ScenarioGenerator::proletariat_robot(&map, &mut rng, &mut timer)
        };

        let mut opts = sim::SimOptions::new("smoke_test");
        opts.alerts = sim::AlertHandler::Silence;
        let mut sim = sim::Sim::new(&map, opts);
        // Bit of an abuse of this, but just need to fix the rng seed.
        let mut rng = sim::SimFlags::for_test("smoke_test").make_rng();
        sim.instantiate(&scenario, &map, &mut rng, &mut timer);
        sim.timed_step(&map, Duration::hours(1), &mut None, &mut timer);
    }
    Ok(())
}

/// Verify all edits under version control can be correctly apply to their map.
fn check_proposals() -> Result<()> {
    let mut timer = Timer::new("check all proposals");
    for name in abstio::list_all_objects(abstio::path("system/proposals")) {
        match abstio::maybe_read_json::<map_model::PermanentMapEdits>(
            abstio::path(format!("system/proposals/{}.json", name)),
            &mut timer,
        ) {
            Ok(perma) => {
                let map = map_model::Map::load_synchronously(perma.map_name.path(), &mut timer);
                if let Err(err) = perma.clone().into_edits(&map) {
                    abstio::write_json(
                        "repair_attempt.json".to_string(),
                        &perma.into_edits_permissive(&map).to_permanent(&map),
                    );
                    anyhow::bail!("{} is out-of-date: {}", name, err);
                }
            }
            Err(err) => {
                anyhow::bail!("{} JSON is broken: {}", name, err);
            }
        }
    }
    Ok(())
}

/// Verify lane-changing behavior is overall reasonable, by asserting all cars and bikes can
/// complete their trip under a time limit.
fn test_lane_changing(map: &Map) -> Result<()> {
    // This uses a fixed RNG seed
    let mut rng = sim::SimFlags::for_test("smoke_test").make_rng();

    // Bit brittle to hardcode IDs here, but it's fast to update
    let north = IntersectionID(7);
    let south = IntersectionID(0);
    let east = IntersectionID(1);
    let west = IntersectionID(3);
    // (origin, destination) pairs
    let mut od = Vec::new();
    for _ in 0..100 {
        od.push((north, south));
        od.push((east, south));
    }
    for _ in 0..100 {
        od.push((north, west));
        od.push((east, west));
    }
    // Shuffling here is critical, since the loop below creates a car/bike and chooses spawn time
    // based on index.
    od.shuffle(&mut rng);

    let mut scenario = Scenario::empty(map, "lane_changing");
    for (idx, (from, to)) in od.into_iter().enumerate() {
        scenario.people.push(PersonSpec {
            orig_id: None,
            trips: vec![IndividTrip::new(
                // Space out the spawn times a bit. If a vehicle tries to spawn and something's in
                // the way, there's a fixed retry time in the simulation that we'll hit.
                Time::START_OF_DAY + Duration::seconds(idx as f64 - 0.5).max(Duration::ZERO),
                TripPurpose::Shopping,
                TripEndpoint::Border(from),
                TripEndpoint::Border(to),
                // About half cars, half bikes
                if idx % 2 == 0 {
                    TripMode::Drive
                } else {
                    TripMode::Bike
                },
            )],
        });
    }
    // Enable to manually watch the scenario
    if false {
        map.save();
        scenario.save();
    }

    let mut opts = sim::SimOptions::new("test_lane_changing");
    opts.alerts = sim::AlertHandler::Silence;
    let mut sim = sim::Sim::new(map, opts);
    let mut rng = sim::SimFlags::for_test("test_lane_changing").make_rng();
    sim.instantiate(&scenario, map, &mut rng, &mut Timer::throwaway());
    while !sim.is_done() {
        sim.tiny_step(map, &mut None);
    }
    // This time limit was determined by watching the scenario manually. This test prevents the
    // time from regressing, which would probably indicate something breaking related to lane
    // selection.
    let limit = Duration::minutes(8) + Duration::seconds(40.0);
    if sim.time() > Time::START_OF_DAY + limit {
        panic!(
            "Lane-changing scenario took {} to complete; it should be under {}",
            sim.time(),
            limit
        );
    }

    Ok(())
}

/// Generate single blocks and merged LTN-style blocks for some maps, counting the number of
/// failures. Store in a goldenfile, so somebody can manually do a visual diff if anything changes.
fn test_blockfinding() -> Result<()> {
    let mut timer = Timer::new("test blockfinding");
    let path = abstio::path("../tests/goldenfiles/blockfinding.txt");
    let mut f = File::create(path)?;

    for name in vec![
        MapName::seattle("montlake"),
        MapName::seattle("downtown"),
        MapName::seattle("lakeslice"),
        MapName::new("us", "phoenix", "tempe"),
        MapName::new("gb", "bristol", "east"),
        MapName::new("gb", "leeds", "north"),
        MapName::new("gb", "london", "camden"),
        MapName::new("gb", "london", "southwark"),
        MapName::new("gb", "manchester", "levenshulme"),
        MapName::new("fr", "lyon", "center"),
        MapName::new("us", "seattle", "north_seattle"),
    ] {
        let map = map_model::Map::load_synchronously(name.path(), &mut timer);
        let mut single_blocks =
            Perimeter::merge_holes(&map, Perimeter::find_all_single_blocks(&map));
        let num_singles_originally = single_blocks.len();
        // Collapse dead-ends first, so results match the LTN tool and blockfinder
        single_blocks.retain(|x| {
            let mut copy = x.clone();
            copy.collapse_deadends();
            copy.to_block(&map).is_ok()
        });
        let num_singles_blockified = single_blocks.len();

        let partitions = Perimeter::partition_by_predicate(single_blocks, |r| {
            map.get_r(r).get_rank() == map_model::osm::RoadRank::Local
        });
        let mut num_partial_merges = 0;
        let mut merged = Vec::new();
        for perimeters in partitions {
            let stepwise_debug = false;
            let newly_merged = Perimeter::merge_all(&map, perimeters, stepwise_debug);
            if newly_merged.len() > 1 {
                num_partial_merges += 1;
            }
            merged.extend(newly_merged);
        }

        let mut num_merged_block_failures = 0;
        for perimeter in merged {
            if perimeter.to_block(&map).is_err() {
                // Note this would break LTN tool!
                num_merged_block_failures += 1;
            }
        }

        writeln!(f, "{}", name.path())?;
        writeln!(f, "    {} single blocks ({} failures to blockify), {} partial merges, {} failures to blockify partitions", num_singles_originally, num_singles_originally - num_singles_blockified, num_partial_merges, num_merged_block_failures)?;
    }
    Ok(())
}

fn ab_test_spurious_diff() -> Result<()> {
    let mut timer = Timer::new("A/B test spurious diff");
    let mut map =
        map_model::Map::load_synchronously(MapName::seattle("montlake").path(), &mut timer);
    let scenario: Scenario =
        abstio::read_binary(abstio::path_scenario(map.get_name(), "weekday"), &mut timer);

    let no_map_edits = run_sim(&map, &scenario, &mut timer);

    // Make some arbitrary map edits
    let mut edits = map.get_edits().clone();
    // It doesn't matter much which road, but if the map changes over time, it could eventually be
    // necessary to fiddle with this
    edits.commands.push(map.edit_road_cmd(RoadID(293), |new| {
        assert_eq!(new.lanes_ltr[1].lt, LaneType::Parking);
        new.lanes_ltr[1].lt = LaneType::Biking;
    }));
    map.must_apply_edits(edits, &mut timer);
    map.recalculate_pathfinding_after_edits(&mut timer);

    let with_map_edits = run_sim(&map, &scenario, &mut timer);

    // Undo the edits
    let mut edits = map.get_edits().clone();
    edits.commands.pop();
    assert!(edits.commands.is_empty());
    map.must_apply_edits(edits, &mut timer);
    map.recalculate_pathfinding_after_edits(&mut timer);

    let after_undoing_map_edits = run_sim(&map, &scenario, &mut timer);

    if no_map_edits.total_trip_duration_seconds == with_map_edits.total_trip_duration_seconds {
        bail!("Changing a parking lane to a bike lane had no effect at all; this is super unlikely; the test is somehow broken");
    }

    // Ignore tiny floating point errors
    // TODO After importing footways, the total difference crept up to a few seconds. Don't know
    // why, not prioritizing it right now.
    if (no_map_edits.total_trip_duration_seconds
        - after_undoing_map_edits.total_trip_duration_seconds)
        .abs()
        > 5.0
    {
        bail!("Undoing map edits resulted in a diff relative to running against the original map: {:?} vs {:?}", no_map_edits, after_undoing_map_edits);
    }

    Ok(())
}

fn run_sim(map: &Map, scenario: &Scenario, timer: &mut Timer) -> PrebakeSummary {
    let mut opts = SimOptions::new("prebaked");
    opts.alerts = AlertHandler::Silence;
    let mut sim = Sim::new(map, opts);
    // Bit of an abuse of this, but just need to fix the rng seed.
    let mut rng = SimFlags::for_test("prebaked").make_rng();
    sim.instantiate(scenario, map, &mut rng, timer);

    // Run until a few hours after the end of the day
    sim.timed_step(
        map,
        sim.get_end_of_day() - Time::START_OF_DAY + Duration::hours(3),
        &mut None,
        timer,
    );

    PrebakeSummary::new(&sim, scenario)
}

/// Describe all public transit routes and keep under version control to spot diffs easily.
fn bus_route_test() -> Result<()> {
    let mut timer = Timer::new("bus route test");
    for name in vec![
        MapName::seattle("arboretum"),
        MapName::new("br", "sao_paulo", "aricanduva"),
    ] {
        let map = map_model::Map::load_synchronously(name.path(), &mut timer);
        let path = abstio::path(format!(
            "../tests/goldenfiles/bus_routes/{}.txt",
            map.get_name().as_filename()
        ));
        let mut f = File::create(path)?;
        for tr in map.all_transit_routes() {
            writeln!(
                f,
                "{} ({}) from {} to {:?}",
                tr.gtfs_id, tr.short_name, tr.start, tr.end_border
            )?;
            for ts in &tr.stops {
                let ts = map.get_ts(*ts);
                writeln!(
                    f,
                    "  {}: {} driving, {} sidewalk",
                    ts.name, ts.driving_pos, ts.sidewalk_pos
                )?;
            }
        }
    }
    Ok(())
}

/// On set maps with bus routes imported, simulate an hour to flush out crashes.
fn bus_test() -> Result<()> {
    let mut timer = Timer::new("bus smoke test");
    for name in vec![
        MapName::seattle("arboretum"),
        MapName::new("us", "san_francisco", "downtown"),
        MapName::new("br", "sao_paulo", "aricanduva"),
        MapName::new("br", "sao_paulo", "center"),
        MapName::new("br", "sao_paulo", "sao_miguel_paulista"),
    ] {
        let map = map_model::Map::load_synchronously(name.path(), &mut timer);
        let mut scenario = Scenario::empty(&map, "bus smoke test");
        scenario.only_seed_buses = None;
        let mut opts = sim::SimOptions::new("smoke_test");
        opts.alerts = sim::AlertHandler::Silence;
        let mut sim = sim::Sim::new(&map, opts);
        // Bit of an abuse of this, but just need to fix the rng seed.
        let mut rng = sim::SimFlags::for_test("smoke_test").make_rng();
        sim.instantiate(&scenario, &map, &mut rng, &mut timer);
        sim.timed_step(&map, Duration::hours(1), &mut None, &mut timer);
    }
    Ok(())
}

/// Check serialized geometry on every map.
fn geometry_test() -> Result<()> {
    let mut timer = Timer::new("check geometry for all maps");
    for name in MapName::list_all_maps_locally() {
        let map = map_model::Map::load_synchronously(name.path(), &mut timer);

        for l in map.all_lanes() {
            let mut sum = Distance::ZERO;
            // This'll crash if duplicate adjacent points snuck in. The sum check is mostly just to
            // make sure the lines actually get iterated. But also, if it fails, we're discovering
            // the same sort of serialization problem!
            for line in l.lane_center_pts.lines() {
                sum += line.length();
            }
            assert_eq!(sum, l.lane_center_pts.length());
        }

        for i in map.all_intersections() {
            for pair in i.polygon.get_outer_ring().points().windows(2) {
                assert_ne!(pair[0], pair[1]);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::geometry_test;
    use super::import_map;
    use super::main;
    use super::test_blockfinding;
    use super::test_lane_changing;
    use super::test_map_importer;
    use tests::get_test_file_path;

    #[test]
    #[ignore]
    fn test_main() -> Result<(), anyhow::Error> {
        main()
    }

    #[test]
    fn run_test_map_importer() -> Result<(), anyhow::Error> {
        test_map_importer()
    }

    #[test]
    #[ignore]
    fn run_geometry_test() -> Result<(), anyhow::Error> {
        geometry_test()
    }

    // // abstutil::logger::setup();

    #[test]
    #[ignore]
    fn run_test_blockfinding() -> Result<(), anyhow::Error> {
        test_blockfinding()
    }

    #[test]
    #[ignore]
    fn run_test_lane_changing() -> Result<(), anyhow::Error> {
        test_lane_changing(&import_map(abstio::path(
            "../tests/input/lane_selection.osm",
        )))
    }

    #[test]
    /// Tests that the `get_test_file_path` convenience function itself works as expected.
    /// Note that this tests is identical to `test_get_test_file_path_ltn_crate`. This is
    /// to assert that the behaviour of `get_test_file_path` is identical from different
    /// locations within the workspace.
    fn test_get_test_file_path_tests_crate() -> Result<(), anyhow::Error> {
        let sample_test_files = vec![
            "input/divided_highway_split.osm",
            "input/left_turn_and_bike_lane.osm",
            "input/multiple_left_turn_lanes.osm",
            "input/false_positive_u_turns.osm",
            "input/turn_restriction_ltn_boundary.osm",
            "goldenfiles/turn_types/divided_highway_split.txt",
            "goldenfiles/turn_types/left_turn_and_bike_lane.txt",
            "goldenfiles/turn_types/multiple_left_turn_lanes.txt",
            "goldenfiles/turn_types/false_positive_u_turns.txt",
            "goldenfiles/turn_types/turn_restriction_ltn_boundary.txt",
        ];

        // test that each of the sample test files can be located
        assert!(sample_test_files
            .iter()
            .all(|f| get_test_file_path(String::from(*f)).is_ok()));

        let sample_test_files = vec!["does_not_exist", "/really/should/not/exist"];

        // test that each of the sample test files cannot be located
        assert!(sample_test_files
            .iter()
            .all(|f| get_test_file_path(String::from(*f)).is_err()));

        Ok(())
    }
}
