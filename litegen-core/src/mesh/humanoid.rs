//! Which joint of a rig is which VRM 1.0 humanoid bone (PixelGenie 3D doc §3D).
//!
//! One table per naming scheme litegen knows. A rig matches a table when all
//! seven core bones — hips, spine, head and both upper arms and upper legs —
//! are present under that table's names; the map then names every bone of the
//! table the rig has. A rig no table matches has no map: litegen never guesses.
//!
//! Sources (re-checked 2026-10-30; fingers, eyes and jaw 2026-11-01):
//! - VRM 1.0 bone names: the normative spec, `VRMC_vrm-1.0/humanoid.md`
//!   (vrm-specification 94e82dd), optional bones included.
//! - Unity: `HumanBodyBones` field names (UnityCsReference bf60283,
//!   Avatar.bindings.cs), 0-53.
//! - Mixamo: the joints of Mixamo-generated FBX ("X Bot", Mixamo, Inc.) — the
//!   67 of the X Bot three.js r170 ships as examples/models/gltf/Xbot.glb;
//!   Adobe publishes no list. Names carry a `mixamorig:` namespace, which Blender
//!   round-trips number (`mixamorig1:`) and containers that cannot spell `:`
//!   write as `mixamorig_`.
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
    // Optional: eyes, jaw, fingers (humanoid.md).
    ("leftEye", "leftEye"), ("rightEye", "rightEye"), ("jaw", "jaw"), ("leftThumbMetacarpal", "leftThumbMetacarpal"),
    ("leftThumbProximal", "leftThumbProximal"), ("leftThumbDistal", "leftThumbDistal"),
    ("leftIndexProximal", "leftIndexProximal"), ("leftIndexIntermediate", "leftIndexIntermediate"),
    ("leftIndexDistal", "leftIndexDistal"), ("leftMiddleProximal", "leftMiddleProximal"),
    ("leftMiddleIntermediate", "leftMiddleIntermediate"), ("leftMiddleDistal", "leftMiddleDistal"),
    ("leftRingProximal", "leftRingProximal"), ("leftRingIntermediate", "leftRingIntermediate"),
    ("leftRingDistal", "leftRingDistal"), ("leftLittleProximal", "leftLittleProximal"),
    ("leftLittleIntermediate", "leftLittleIntermediate"), ("leftLittleDistal", "leftLittleDistal"),
    ("rightThumbMetacarpal", "rightThumbMetacarpal"), ("rightThumbProximal", "rightThumbProximal"),
    ("rightThumbDistal", "rightThumbDistal"), ("rightIndexProximal", "rightIndexProximal"),
    ("rightIndexIntermediate", "rightIndexIntermediate"), ("rightIndexDistal", "rightIndexDistal"),
    ("rightMiddleProximal", "rightMiddleProximal"), ("rightMiddleIntermediate", "rightMiddleIntermediate"),
    ("rightMiddleDistal", "rightMiddleDistal"), ("rightRingProximal", "rightRingProximal"),
    ("rightRingIntermediate", "rightRingIntermediate"), ("rightRingDistal", "rightRingDistal"),
    ("rightLittleProximal", "rightLittleProximal"), ("rightLittleIntermediate", "rightLittleIntermediate"),
    ("rightLittleDistal", "rightLittleDistal"),
];

const UNITY: Table = &[
    ("hips", "Hips"), ("spine", "Spine"), ("chest", "Chest"), ("upperChest", "UpperChest"), ("neck", "Neck"), ("head", "Head"),
    ("leftShoulder", "LeftShoulder"), ("leftUpperArm", "LeftUpperArm"), ("leftLowerArm", "LeftLowerArm"), ("leftHand", "LeftHand"),
    ("rightShoulder", "RightShoulder"), ("rightUpperArm", "RightUpperArm"), ("rightLowerArm", "RightLowerArm"), ("rightHand", "RightHand"),
    ("leftUpperLeg", "LeftUpperLeg"), ("leftLowerLeg", "LeftLowerLeg"), ("leftFoot", "LeftFoot"), ("leftToes", "LeftToes"),
    ("rightUpperLeg", "RightUpperLeg"), ("rightLowerLeg", "RightLowerLeg"), ("rightFoot", "RightFoot"), ("rightToes", "RightToes"),
    // HumanBodyBones 21-53; a thumb counts Proximal, Intermediate, Distal from the hand.
    ("leftEye", "LeftEye"), ("rightEye", "RightEye"), ("jaw", "Jaw"), ("leftThumbMetacarpal", "LeftThumbProximal"),
    ("leftThumbProximal", "LeftThumbIntermediate"), ("leftThumbDistal", "LeftThumbDistal"),
    ("leftIndexProximal", "LeftIndexProximal"), ("leftIndexIntermediate", "LeftIndexIntermediate"),
    ("leftIndexDistal", "LeftIndexDistal"), ("leftMiddleProximal", "LeftMiddleProximal"),
    ("leftMiddleIntermediate", "LeftMiddleIntermediate"), ("leftMiddleDistal", "LeftMiddleDistal"),
    ("leftRingProximal", "LeftRingProximal"), ("leftRingIntermediate", "LeftRingIntermediate"),
    ("leftRingDistal", "LeftRingDistal"), ("leftLittleProximal", "LeftLittleProximal"),
    ("leftLittleIntermediate", "LeftLittleIntermediate"), ("leftLittleDistal", "LeftLittleDistal"),
    ("rightThumbMetacarpal", "RightThumbProximal"), ("rightThumbProximal", "RightThumbIntermediate"),
    ("rightThumbDistal", "RightThumbDistal"), ("rightIndexProximal", "RightIndexProximal"),
    ("rightIndexIntermediate", "RightIndexIntermediate"), ("rightIndexDistal", "RightIndexDistal"),
    ("rightMiddleProximal", "RightMiddleProximal"), ("rightMiddleIntermediate", "RightMiddleIntermediate"),
    ("rightMiddleDistal", "RightMiddleDistal"), ("rightRingProximal", "RightRingProximal"),
    ("rightRingIntermediate", "RightRingIntermediate"), ("rightRingDistal", "RightRingDistal"),
    ("rightLittleProximal", "RightLittleProximal"), ("rightLittleIntermediate", "RightLittleIntermediate"),
    ("rightLittleDistal", "RightLittleDistal"),
];

/// Mixamo's names without the `mixamorig:` namespace (matched with it).
const MIXAMO: Table = &[
    ("hips", "Hips"), ("spine", "Spine"), ("chest", "Spine1"), ("upperChest", "Spine2"), ("neck", "Neck"), ("head", "Head"),
    ("leftShoulder", "LeftShoulder"), ("leftUpperArm", "LeftArm"), ("leftLowerArm", "LeftForeArm"), ("leftHand", "LeftHand"),
    ("rightShoulder", "RightShoulder"), ("rightUpperArm", "RightArm"), ("rightLowerArm", "RightForeArm"), ("rightHand", "RightHand"),
    ("leftUpperLeg", "LeftUpLeg"), ("leftLowerLeg", "LeftLeg"), ("leftFoot", "LeftFoot"), ("leftToes", "LeftToeBase"),
    ("rightUpperLeg", "RightUpLeg"), ("rightLowerLeg", "RightLeg"), ("rightFoot", "RightFoot"), ("rightToes", "RightToeBase"),
    // X Bot's eyes and fingers (no jaw); 1, 2, 3 out from the hand, 4 the tip.
    ("leftEye", "LeftEye"), ("rightEye", "RightEye"), ("leftThumbMetacarpal", "LeftHandThumb1"),
    ("leftThumbProximal", "LeftHandThumb2"), ("leftThumbDistal", "LeftHandThumb3"),
    ("leftIndexProximal", "LeftHandIndex1"), ("leftIndexIntermediate", "LeftHandIndex2"),
    ("leftIndexDistal", "LeftHandIndex3"), ("leftMiddleProximal", "LeftHandMiddle1"),
    ("leftMiddleIntermediate", "LeftHandMiddle2"), ("leftMiddleDistal", "LeftHandMiddle3"),
    ("leftRingProximal", "LeftHandRing1"), ("leftRingIntermediate", "LeftHandRing2"),
    ("leftRingDistal", "LeftHandRing3"), ("leftLittleProximal", "LeftHandPinky1"),
    ("leftLittleIntermediate", "LeftHandPinky2"), ("leftLittleDistal", "LeftHandPinky3"),
    ("rightThumbMetacarpal", "RightHandThumb1"), ("rightThumbProximal", "RightHandThumb2"),
    ("rightThumbDistal", "RightHandThumb3"), ("rightIndexProximal", "RightHandIndex1"),
    ("rightIndexIntermediate", "RightHandIndex2"), ("rightIndexDistal", "RightHandIndex3"),
    ("rightMiddleProximal", "RightHandMiddle1"), ("rightMiddleIntermediate", "RightHandMiddle2"),
    ("rightMiddleDistal", "RightHandMiddle3"), ("rightRingProximal", "RightHandRing1"),
    ("rightRingIntermediate", "RightHandRing2"), ("rightRingDistal", "RightHandRing3"),
    ("rightLittleProximal", "RightHandPinky1"), ("rightLittleIntermediate", "RightHandPinky2"),
    ("rightLittleDistal", "RightHandPinky3"),
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

/// A joint name without its `mixamorig` namespace, or `None` when it has
/// none: `mixamorig:` as Mixamo writes it, `mixamorig1:` as Blender
/// re-imports number it, `mixamorig_` as containers that cannot spell `:`
/// write it (Collada ids, USD prim names), or no separator at all before an
/// upper-case name, as some tools strip it.
fn strip_mixamo_namespace(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("mixamorig")?;
    let rest = &rest[rest.bytes().take_while(u8::is_ascii_digit).count()..];
    match rest.strip_prefix(':').or_else(|| rest.strip_prefix('_')) {
        Some(bare) => Some(bare),
        None => rest.starts_with(|c: char| c.is_ascii_uppercase()).then_some(rest),
    }
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

/// The schemes litegen names a humanoid rig's joints in itself (3D doc §3B):
/// those with a table to name them from.
pub const RENAMES_TO: [&str; 3] = ["vrm", "unity", "mixamo"];

/// The renames `(joint, new name)` that give a humanoid rig `scheme`'s joint
/// names — every bone its own table and the scheme's both name — leaving the
/// rest as they are. `None` when no table matches the rig or `scheme` is not
/// one of [`RENAMES_TO`].
pub fn renames_to(scheme: &str, joints: &[String]) -> Option<Vec<(String, String)>> {
    let target: Table = match scheme {
        "vrm" => VRM,
        "unity" => UNITY,
        "mixamo" => MIXAMO,
        _ => return None,
    };
    let (_, map) = humanoid_map(joints)?;
    Some(
        map.into_iter()
            .filter_map(|(bone, joint)| {
                let name = target.iter().find(|(b, _)| *b == bone)?.1;
                let name = if scheme == "mixamo" { format!("mixamorig:{name}") } else { name.to_string() };
                (name != joint).then_some((joint, name))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rig_litegen_can_read_is_renamed_bone_for_bone_into_another_scheme() {
        let meshy = names(&["Hips", "Spine02", "Spine01", "Spine", "neck", "Head", "LeftArm", "RightArm", "LeftUpLeg", "RightUpLeg", "LeftHand"]);
        let renames = renames_to("unity", &meshy).unwrap();
        let to = |from: &str| renames.iter().find(|(f, _)| f == from).map(|(_, t)| t.as_str());
        assert_eq!(
            (to("Spine02"), to("Spine01"), to("Spine"), to("neck"), to("LeftArm"), to("LeftUpLeg")),
            (Some("Spine"), Some("Chest"), Some("UpperChest"), Some("Neck"), Some("LeftUpperArm"), Some("LeftUpperLeg"))
        );
        assert!(renames.iter().all(|(f, t)| f != t), "no rename to the same name");
        let mock = names(&["hips", "torso", "head", "arm_l", "arm_r", "leg_l", "leg_r"]);
        let mixamo = renames_to("mixamo", &mock).unwrap();
        assert!(mixamo.contains(&("torso".into(), "mixamorig:Spine".into())), "{mixamo:?}");
        assert!(mixamo.contains(&("arm_l".into(), "mixamorig:LeftArm".into())), "{mixamo:?}");
        assert_eq!(renames_to("vrm", &names(&["root", "bone1"])), None, "a rig no table matches");
        assert_eq!(renames_to("unreal", &mock), None, "no table names UE5 Manny's bones");
        assert_eq!(renames_to("vrm", &names(&["hips", "spine", "head", "leftUpperArm", "rightUpperArm", "leftUpperLeg", "rightUpperLeg"])), Some(vec![]), "already VRM");
    }

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
        // Containers that cannot spell `:` (Collada's NCName ids, USD prim
        // names) write `_`; some tools drop the separator.
        for sep in ["mixamorig_", "mixamorig1_", "mixamorig"] {
            let respelled: Vec<String> = joints.iter().map(|j| j.replace("mixamorig:", sep)).collect();
            assert_eq!(humanoid_map(&respelled).map(|m| m.0), Some("mixamo"), "{sep}");
        }
        // A word that only starts like the namespace is not it.
        assert_eq!(strip_mixamo_namespace("mixamorigs:Hips"), None);
        assert_eq!(strip_mixamo_namespace("mixamorighips"), None);
    }

    /// a3 #7 (2026-10-31 delivery review): fingers, eyes and the jaw map and
    /// rename too — VRM 1.0's optional bones (humanoid.md), Unity's
    /// HumanBodyBones 21-53 (Avatar.bindings.cs), and the X Bot three.js r170
    /// ships as examples/models/gltf/Xbot.glb (67 joints, `LeftHandThumb1-4`
    /// … `LeftHandPinky1-4`, `LeftEye`/`RightEye`, no jaw). VRM 1.0 counts a
    /// thumb Metacarpal, Proximal, Distal out from the hand (its Parent Bone
    /// column); Unity counts Proximal, Intermediate, Distal and Mixamo 1, 2, 3:
    /// the same joints, by their place in the chain. Mixamo's 4 is the tip,
    /// which no humanoid names.
    #[test]
    fn fingers_eyes_and_jaw_map_and_rename() {
        let xbot = names(&[
            "mixamorig:Hips", "mixamorig:Spine", "mixamorig:Spine1", "mixamorig:Spine2", "mixamorig:Neck", "mixamorig:Head",
            "mixamorig:HeadTop_End", "mixamorig:LeftEye", "mixamorig:RightEye",
            "mixamorig:LeftShoulder", "mixamorig:LeftArm", "mixamorig:LeftForeArm", "mixamorig:LeftHand",
            "mixamorig:LeftHandThumb1", "mixamorig:LeftHandThumb2", "mixamorig:LeftHandThumb3", "mixamorig:LeftHandThumb4",
            "mixamorig:LeftHandIndex1", "mixamorig:LeftHandIndex2", "mixamorig:LeftHandIndex3",
            "mixamorig:RightShoulder", "mixamorig:RightArm", "mixamorig:RightForeArm", "mixamorig:RightHand",
            "mixamorig:RightHandPinky1", "mixamorig:RightHandPinky2", "mixamorig:RightHandPinky3",
            "mixamorig:LeftUpLeg", "mixamorig:LeftLeg", "mixamorig:LeftFoot", "mixamorig:LeftToeBase",
            "mixamorig:RightUpLeg", "mixamorig:RightLeg", "mixamorig:RightFoot", "mixamorig:RightToeBase",
        ]);
        let (table, map) = humanoid_map(&xbot).unwrap();
        assert_eq!(table, "mixamo");
        let get = |b: &str| map.iter().find(|(k, _)| k == b).map(|(_, v)| v.as_str());
        assert_eq!(get("leftThumbMetacarpal"), Some("mixamorig:LeftHandThumb1"));
        assert_eq!(get("leftThumbDistal"), Some("mixamorig:LeftHandThumb3"));
        assert_eq!(get("leftIndexIntermediate"), Some("mixamorig:LeftHandIndex2"));
        assert_eq!(get("rightLittleDistal"), Some("mixamorig:RightHandPinky3"));
        assert_eq!(get("leftEye"), Some("mixamorig:LeftEye"));
        let unity = renames_to("unity", &xbot).unwrap();
        for (from, to) in [
            ("mixamorig:LeftHandThumb1", "LeftThumbProximal"),
            ("mixamorig:LeftHandThumb2", "LeftThumbIntermediate"),
            ("mixamorig:LeftHandThumb3", "LeftThumbDistal"),
            ("mixamorig:RightHandPinky3", "RightLittleDistal"),
            ("mixamorig:RightEye", "RightEye"),
        ] {
            assert!(unity.contains(&(from.to_string(), to.to_string())), "{from} → {to}: {unity:?}");
        }
        let vrm = renames_to("vrm", &xbot).unwrap();
        assert!(vrm.contains(&("mixamorig:LeftHandThumb1".to_string(), "leftThumbMetacarpal".to_string())), "{vrm:?}");
        // Unity's jaw has a VRM name and no Mixamo one: kept as it is.
        let mut with_jaw: Vec<String> = unity.iter().map(|(_, to)| to.clone()).collect();
        with_jaw.extend(["Hips", "Spine", "Head", "LeftUpperArm", "RightUpperArm", "LeftUpperLeg", "RightUpperLeg", "Jaw"].map(String::from));
        let back = renames_to("vrm", &with_jaw).unwrap();
        assert!(back.contains(&("Jaw".to_string(), "jaw".to_string())), "{back:?}");
        assert!(!renames_to("mixamo", &with_jaw).unwrap().iter().any(|(from, _)| from == "Jaw"));
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
