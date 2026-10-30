//! Which joint of a rig is which VRM 1.0 humanoid bone (PixelGenie 3D doc §3D).
//!
//! One table per naming scheme litegen knows. A rig matches a table when all
//! seven core bones — hips, spine, head and both upper arms and upper legs —
//! are present under that table's names; the map then names every bone of the
//! table the rig has. A rig no table matches has no map: litegen never guesses.
//!
//! Sources (re-checked 2026-10-30):
//! - VRM 1.0 bone names: the normative spec, `VRMC_vrm-1.0/humanoid.md`.
//! - Unity: `HumanBodyBones` field names (UnityCsReference, Avatar.bindings.cs).
//! - Mixamo: the joints of Mixamo-generated FBX ("X Bot", Mixamo, Inc.); Adobe
//!   publishes no list. Names carry a `mixamorig:` namespace, which Blender
//!   round-trips number (`mixamorig1:`).
//! - Meshy: recorded from `fal-ai/meshy/rigging`'s example output — a spine
//!   numbered in reverse (Hips → Spine02 → Spine01 → Spine) and a lower-case
//!   `neck`.
//!
//! Not built, because no official list was found: UE5 Manny (Epic's pages name
//! only some of its bones) and Tripo (publishes no joint names). A table is
//! added when a source is.

/// The bones every humanoid map must have.
pub const CORE_BONES: [&str; 7] = ["hips", "spine", "head", "leftUpperArm", "rightUpperArm", "leftUpperLeg", "rightUpperLeg"];

/// (VRM bone, joint name) pairs, per table.
type Table = &'static [(&'static str, &'static str)];

const VRM: Table = &[
    ("hips", "hips"), ("spine", "spine"), ("chest", "chest"), ("upperChest", "upperChest"), ("neck", "neck"), ("head", "head"),
    ("leftShoulder", "leftShoulder"), ("leftUpperArm", "leftUpperArm"), ("leftLowerArm", "leftLowerArm"), ("leftHand", "leftHand"),
    ("rightShoulder", "rightShoulder"), ("rightUpperArm", "rightUpperArm"), ("rightLowerArm", "rightLowerArm"), ("rightHand", "rightHand"),
    ("leftUpperLeg", "leftUpperLeg"), ("leftLowerLeg", "leftLowerLeg"), ("leftFoot", "leftFoot"), ("leftToes", "leftToes"),
    ("rightUpperLeg", "rightUpperLeg"), ("rightLowerLeg", "rightLowerLeg"), ("rightFoot", "rightFoot"), ("rightToes", "rightToes"),
];

const UNITY: Table = &[
    ("hips", "Hips"), ("spine", "Spine"), ("chest", "Chest"), ("upperChest", "UpperChest"), ("neck", "Neck"), ("head", "Head"),
    ("leftShoulder", "LeftShoulder"), ("leftUpperArm", "LeftUpperArm"), ("leftLowerArm", "LeftLowerArm"), ("leftHand", "LeftHand"),
    ("rightShoulder", "RightShoulder"), ("rightUpperArm", "RightUpperArm"), ("rightLowerArm", "RightLowerArm"), ("rightHand", "RightHand"),
    ("leftUpperLeg", "LeftUpperLeg"), ("leftLowerLeg", "LeftLowerLeg"), ("leftFoot", "LeftFoot"), ("leftToes", "LeftToes"),
    ("rightUpperLeg", "RightUpperLeg"), ("rightLowerLeg", "RightLowerLeg"), ("rightFoot", "RightFoot"), ("rightToes", "RightToes"),
];

/// Mixamo's names without the `mixamorig:` namespace (matched with it).
const MIXAMO: Table = &[
    ("hips", "Hips"), ("spine", "Spine"), ("chest", "Spine1"), ("upperChest", "Spine2"), ("neck", "Neck"), ("head", "Head"),
    ("leftShoulder", "LeftShoulder"), ("leftUpperArm", "LeftArm"), ("leftLowerArm", "LeftForeArm"), ("leftHand", "LeftHand"),
    ("rightShoulder", "RightShoulder"), ("rightUpperArm", "RightArm"), ("rightLowerArm", "RightForeArm"), ("rightHand", "RightHand"),
    ("leftUpperLeg", "LeftUpLeg"), ("leftLowerLeg", "LeftLeg"), ("leftFoot", "LeftFoot"), ("leftToes", "LeftToeBase"),
    ("rightUpperLeg", "RightUpLeg"), ("rightLowerLeg", "RightLeg"), ("rightFoot", "RightFoot"), ("rightToes", "RightToeBase"),
];

const MESHY: Table = &[
    ("hips", "Hips"), ("spine", "Spine02"), ("chest", "Spine01"), ("upperChest", "Spine"), ("neck", "neck"), ("head", "Head"),
    ("leftShoulder", "LeftShoulder"), ("leftUpperArm", "LeftArm"), ("leftLowerArm", "LeftForeArm"), ("leftHand", "LeftHand"),
    ("rightShoulder", "RightShoulder"), ("rightUpperArm", "RightArm"), ("rightLowerArm", "RightForeArm"), ("rightHand", "RightHand"),
    ("leftUpperLeg", "LeftUpLeg"), ("leftLowerLeg", "LeftLeg"), ("leftFoot", "LeftFoot"), ("leftToes", "LeftToeBase"),
    ("rightUpperLeg", "RightUpLeg"), ("rightLowerLeg", "RightLeg"), ("rightFoot", "RightFoot"), ("rightToes", "RightToeBase"),
];

/// `mock/rig-3d`'s seven joints (3D doc §3I).
const LITEGEN_MOCK: Table = &[
    ("hips", "hips"), ("spine", "torso"), ("head", "head"),
    ("leftUpperArm", "arm_l"), ("rightUpperArm", "arm_r"), ("leftUpperLeg", "leg_l"), ("rightUpperLeg", "leg_r"),
];

/// Tables in the order they are tried; the first that matches wins. Mixamo
/// needs its namespace, so a bare Meshy rig (which shares Mixamo's limb names)
/// never reads as Mixamo.
const TABLES: [(&str, Table, bool); 5] = [
    ("vrm", VRM, false),
    ("unity", UNITY, false),
    ("mixamo", MIXAMO, true),
    ("meshy", MESHY, false),
    ("litegen-mock", LITEGEN_MOCK, false),
];

/// A joint name without a `mixamorig:` / `mixamorig<N>:` namespace, or `None`
/// when it has none.
fn strip_mixamo_namespace(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("mixamorig")?;
    let colon = rest.find(':')?;
    rest[..colon].bytes().all(|b| b.is_ascii_digit()).then(|| &rest[colon + 1..])
}

/// The rig's humanoid map: the matching table's name, and (VRM bone, joint
/// name) for every bone of that table the rig has. `None` when no table
/// matches the seven core bones.
pub fn humanoid_map(joints: &[String]) -> Option<(&'static str, Vec<(String, String)>)> {
    for (name, table, namespaced) in TABLES {
        let find = |wanted: &str| -> Option<&String> {
            joints.iter().find(|j| {
                if namespaced {
                    strip_mixamo_namespace(j) == Some(wanted)
                } else {
                    j.as_str() == wanted
                }
            })
        };
        let core_present = CORE_BONES.iter().all(|bone| {
            table.iter().find(|(b, _)| b == bone).is_some_and(|(_, joint)| find(joint).is_some())
        });
        if !core_present {
            continue;
        }
        let map = table
            .iter()
            .filter_map(|(bone, joint)| find(joint).map(|j| (bone.to_string(), j.clone())))
            .collect();
        return Some((name, map));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_litegen_mock_rig_maps_as_section_3i_says() {
        let (table, map) = humanoid_map(&names(&["hips", "torso", "head", "arm_l", "arm_r", "leg_l", "leg_r"])).unwrap();
        assert_eq!(table, "litegen-mock");
        let get = |b: &str| map.iter().find(|(k, _)| k == b).map(|(_, v)| v.as_str());
        assert_eq!(get("hips"), Some("hips"));
        assert_eq!(get("spine"), Some("torso"));
        assert_eq!(get("head"), Some("head"));
        assert_eq!(get("leftUpperArm"), Some("arm_l"));
        assert_eq!(get("rightUpperArm"), Some("arm_r"));
        assert_eq!(get("leftUpperLeg"), Some("leg_l"));
        assert_eq!(get("rightUpperLeg"), Some("leg_r"));
        assert_eq!(map.len(), 7);
    }

    #[test]
    fn vrm_names_map_to_themselves() {
        let (table, map) = humanoid_map(&names(&["hips", "spine", "head", "leftUpperArm", "rightUpperArm", "leftUpperLeg", "rightUpperLeg"])).unwrap();
        assert_eq!(table, "vrm");
        assert!(map.iter().all(|(k, v)| k == v));
    }

    #[test]
    fn a_mixamo_rig_maps_through_its_namespace_prefix() {
        let joints = names(&[
            "mixamorig:Hips", "mixamorig:Spine", "mixamorig:Spine1", "mixamorig:Spine2", "mixamorig:Neck", "mixamorig:Head",
            "mixamorig:LeftShoulder", "mixamorig:LeftArm", "mixamorig:LeftForeArm", "mixamorig:LeftHand",
            "mixamorig:RightShoulder", "mixamorig:RightArm", "mixamorig:RightForeArm", "mixamorig:RightHand",
            "mixamorig:LeftUpLeg", "mixamorig:LeftLeg", "mixamorig:LeftFoot", "mixamorig:LeftToeBase",
            "mixamorig:RightUpLeg", "mixamorig:RightLeg", "mixamorig:RightFoot", "mixamorig:RightToeBase",
        ]);
        let (table, map) = humanoid_map(&joints).unwrap();
        assert_eq!(table, "mixamo");
        let get = |b: &str| map.iter().find(|(k, _)| k == b).map(|(_, v)| v.as_str());
        assert_eq!(get("spine"), Some("mixamorig:Spine"));
        assert_eq!(get("chest"), Some("mixamorig:Spine1"));
        assert_eq!(get("leftUpperArm"), Some("mixamorig:LeftArm"));
        assert_eq!(get("leftLowerLeg"), Some("mixamorig:LeftLeg"));
        // Blender re-imports number the namespace: mixamorig1:Hips.
        let renumbered: Vec<String> = joints.iter().map(|j| j.replace("mixamorig:", "mixamorig1:")).collect();
        assert_eq!(humanoid_map(&renumbered).unwrap().0, "mixamo");
    }

    #[test]
    fn a_meshy_rig_is_told_apart_from_mixamo_by_its_reversed_spine() {
        // Recorded from fal-ai/meshy/rigging's example output, 2026-10-30.
        let joints = names(&[
            "Hips", "LeftUpLeg", "LeftLeg", "LeftFoot", "LeftToeBase", "RightUpLeg", "RightLeg", "RightFoot", "RightToeBase",
            "Spine02", "Spine01", "Spine", "LeftShoulder", "LeftArm", "LeftForeArm", "LeftHand",
            "RightShoulder", "RightArm", "RightForeArm", "RightHand", "neck", "Head", "head_end", "headfront",
        ]);
        let (table, map) = humanoid_map(&joints).unwrap();
        assert_eq!(table, "meshy");
        let get = |b: &str| map.iter().find(|(k, _)| k == b).map(|(_, v)| v.as_str());
        assert_eq!(get("spine"), Some("Spine02"), "Hips → Spine02 → Spine01 → Spine");
        assert_eq!(get("upperChest"), Some("Spine"));
        assert_eq!(get("neck"), Some("neck"));
    }

    #[test]
    fn a_unity_humanoid_rig_maps() {
        let joints = names(&["Hips", "Spine", "Chest", "Neck", "Head", "LeftUpperArm", "LeftLowerArm", "LeftHand", "RightUpperArm", "RightLowerArm", "RightHand", "LeftUpperLeg", "LeftLowerLeg", "LeftFoot", "RightUpperLeg", "RightLowerLeg", "RightFoot"]);
        assert_eq!(humanoid_map(&joints).unwrap().0, "unity");
    }

    #[test]
    fn a_rig_no_table_matches_is_never_guessed() {
        assert!(humanoid_map(&names(&["root", "bone_1", "bone_2"])).is_none());
        // A partial match (no legs) is not a humanoid either.
        assert!(humanoid_map(&names(&["hips", "spine", "head", "leftUpperArm", "rightUpperArm"])).is_none());
    }
}
