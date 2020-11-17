//! Integration tests

use std::fs::File;
use std::io::Write;

use rand::seq::SliceRandom;

use abstutil::{MapName, Timer};
use geom::{Duration, Time};
use map_model::{LaneID, Map, PathConstraints};
use sim::{DrivingGoal, IndividTrip, PersonID, PersonSpec, Scenario, SpawnTrip, TripPurpose};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    test_lane_changing(&import_map(abstutil::path(
        "../tests/input/lane_selection.osm",
    )))?;
    test_map_importer()?;
    check_proposals()?;
    smoke_test()?;
    Ok(())
}

/// Test the map pipeline by importing simple, handcrafted .osm files, then emitting goldenfiles
/// that summarize part of the generated map. Keep the goldenfiles under version control to notice
/// when they change. The goldenfiles (and changes to them) themselves aren't easy to understand,
/// but the test maps are.
fn test_map_importer() -> Result<(), std::io::Error> {
    for name in vec!["left_turn_and_bike_lane", "multiple_left_turn_lanes"] {
        // TODO It's kind of a hack to reference the crate's directory relative to the data dir.
        let map = import_map(abstutil::path(format!("../tests/input/{}.osm", name)));
        // Enable to debug the result wih the normal GUI
        if false {
            map.save();
        }
        println!("Producing goldenfiles for {}", map.get_name().describe());
        dump_turn_goldenfile(&map)?;
    }
    Ok(())
}

/// Run the contents of a .osm through the full map importer with default options.
fn import_map(path: String) -> Map {
    let mut timer = abstutil::Timer::new("convert synthetic map");
    let raw = convert_osm::convert(
        convert_osm::Options {
            name: MapName::new("oneshot", &abstutil::basename(&path)),
            osm_input: path,
            clip: None,
            map_config: map_model::MapConfig {
                driving_side: map_model::DrivingSide::Right,
                bikes_can_use_bus_lanes: true,
                inferred_sidewalks: true,
            },
            onstreet_parking: convert_osm::OnstreetParking::JustOSM,
            public_offstreet_parking: convert_osm::PublicOffstreetParking::None,
            private_offstreet_parking: convert_osm::PrivateOffstreetParking::FixedPerBldg(0),
            elevation: None,
            include_railroads: true,
        },
        &mut timer,
    );
    let map = Map::create_from_raw(raw, true, true, &mut timer);
    map
}

/// Verify what turns are generated by writing (from lane, to lane, turn type).
fn dump_turn_goldenfile(map: &Map) -> Result<(), std::io::Error> {
    let path = abstutil::path(format!("../tests/goldenfiles/{}.txt", map.get_name().map));
    let mut f = File::create(path)?;
    for (_, t) in map.all_turns() {
        writeln!(f, "{} is a {:?}", t.id, t.turn_type)?;
    }
    Ok(())
}

/// Simulate an hour on every map.
fn smoke_test() -> Result<(), std::io::Error> {
    let mut timer = Timer::new("run a smoke-test for all maps");
    for name in MapName::list_all_maps() {
        let map = map_model::Map::new(name.path(), &mut timer);
        let scenario = if map.get_city_name() == "seattle" {
            abstutil::read_binary(abstutil::path_scenario(&name, "weekday"), &mut timer)
        } else {
            let mut rng = sim::SimFlags::for_test("smoke_test").make_rng();
            sim::ScenarioGenerator::proletariat_robot(&map, &mut rng, &mut timer)
        };

        let mut opts = sim::SimOptions::new("smoke_test");
        opts.alerts = sim::AlertHandler::Silence;
        let mut sim = sim::Sim::new(&map, opts, &mut timer);
        // Bit of an abuse of this, but just need to fix the rng seed.
        let mut rng = sim::SimFlags::for_test("smoke_test").make_rng();
        scenario.instantiate(&mut sim, &map, &mut rng, &mut timer);
        sim.timed_step(&map, Duration::hours(1), &mut None, &mut timer);

        if (name.city == "seattle"
            && vec!["downtown", "lakeslice", "montlake", "udistrict"].contains(&name.map.as_str()))
            || name == MapName::new("krakow", "center")
        {
            dump_route_goldenfile(&map)?;
        }
    }
    Ok(())
}

/// Describe all public transit routes and keep under version control to spot diffs easily.
fn dump_route_goldenfile(map: &map_model::Map) -> Result<(), std::io::Error> {
    let path = abstutil::path(format!(
        "route_goldenfiles/{}.txt",
        map.get_name().as_filename()
    ));
    let mut f = File::create(path)?;
    for br in map.all_bus_routes() {
        writeln!(
            f,
            "{} from {} to {:?}",
            br.osm_rel_id, br.start, br.end_border
        )?;
        for bs in &br.stops {
            let bs = map.get_bs(*bs);
            writeln!(
                f,
                "  {}: {} driving, {} sidewalk",
                bs.name, bs.driving_pos, bs.sidewalk_pos
            )?;
        }
    }
    Ok(())
}

/// Verify all edits under version control can be correctly apply to their map.
fn check_proposals() -> Result<(), String> {
    let mut timer = Timer::new("check all proposals");
    for name in abstutil::list_all_objects(abstutil::path("system/proposals")) {
        match abstutil::maybe_read_json::<map_model::PermanentMapEdits>(
            abstutil::path(format!("system/proposals/{}.json", name)),
            &mut timer,
        ) {
            Ok(perma) => {
                let map = map_model::Map::new(perma.map_name.path(), &mut timer);
                if let Err(err) = perma.clone().to_edits(&map) {
                    abstutil::write_json(
                        "repair_attempt.json".to_string(),
                        &perma.to_edits_permissive(&map).to_permanent(&map),
                    );
                    return Err(format!("{} is out-of-date: {}", name, err));
                }
            }
            Err(err) => {
                return Err(format!("{} JSON is broken: {}", name, err));
            }
        }
    }
    Ok(())
}

/// Verify lane-chaging behavior is overall reasonable, by asserting all cars and bikes can
/// complete their trip under a time limit.
fn test_lane_changing(map: &Map) -> Result<(), String> {
    // This uses a fixed RNG seed
    let mut rng = sim::SimFlags::for_test("smoke_test").make_rng();

    // Bit brittle to hardcode IDs here, but it's fast to update
    let north = map.get_l(LaneID(23)).get_directed_parent(map);
    let south = DrivingGoal::end_at_border(
        map.get_l(LaneID(31)).get_directed_parent(map),
        PathConstraints::Car,
        map,
    )
    .unwrap();
    let east = map.get_l(LaneID(6)).get_directed_parent(map);
    let west = DrivingGoal::end_at_border(
        map.get_l(LaneID(15)).get_directed_parent(map),
        PathConstraints::Car,
        map,
    )
    .unwrap();
    // (origin, destination) pairs
    let mut od = Vec::new();
    for _ in 0..100 {
        od.push((north, south.clone()));
        od.push((east, south.clone()));
    }
    for _ in 0..100 {
        od.push((north, west.clone()));
        od.push((east, west.clone()));
    }
    // Shuffling here is critical, since the loop below creates a car/bike and chooses spawn time
    // based on index.
    od.shuffle(&mut rng);

    let mut scenario = Scenario::empty(map, "lane_changing");
    for (from, to) in od {
        let id = PersonID(scenario.people.len());
        scenario.people.push(PersonSpec {
            id,
            orig_id: None,
            trips: vec![IndividTrip::new(
                // Space out the spawn times a bit. If a vehicle tries to spawn and something's in
                // the way, there's a fixed retry time in the simulation that we'll hit.
                Time::START_OF_DAY + Duration::seconds(id.0 as f64 - 0.5).max(Duration::ZERO),
                TripPurpose::Shopping,
                SpawnTrip::FromBorder {
                    dr: from,
                    goal: to,
                    // About half cars, half bikes
                    is_bike: id.0 % 2 == 0,
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
    let mut sim = sim::Sim::new(&map, opts, &mut Timer::throwaway());
    let mut rng = sim::SimFlags::for_test("test_lane_changing").make_rng();
    scenario.instantiate(&mut sim, &map, &mut rng, &mut Timer::throwaway());
    while !sim.is_done() {
        sim.tiny_step(&map, &mut None);
    }
    // This time limit was determined by watching the scenario manually. This test prevents the
    // time from regressing, which would probably indicate something breaking related to lane
    // selection.
    let limit = Duration::minutes(8) + Duration::seconds(10.0);
    if sim.time() > Time::START_OF_DAY + limit {
        panic!(
            "Lane-changing scenario took {} to complete; it should be under {}",
            sim.time(),
            limit
        );
    }

    Ok(())
}
