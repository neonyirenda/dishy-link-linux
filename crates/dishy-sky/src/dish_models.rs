//! The per-kit dish meshes (spec 0183): Electron's converted CAD models,
//! exported by upstream `scripts/export-dish-models.mts` and bundled into the binary.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// Where a motorised mount hinges: vertices from `base_vertex` on are the foot.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Joint {
    pub base_vertex: usize,
    pub pivot: [f64; 3],
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    scale: f64,
    long_axis_mm: f64,
    parts: Vec<(usize, usize, f64)>,
    vertex_count: usize,
    triangle_count: usize,
    joint: Option<Joint>,
    keep_camera_outside: bool,
    beam_origin_mm: Option<[f64; 3]>,
}

/// One kit's mesh, decoded.
#[derive(Debug, Clone)]
pub struct DishModelMesh {
    /// Divisor taking the stored ints back to millimetres.
    pub scale: f64,
    pub long_axis_mm: f64,
    /// `(first triangle, triangle count, tint)` per solid.
    pub parts: Vec<(usize, usize, f64)>,
    /// xyz per vertex, in stored units.
    pub positions: Vec<i16>,
    /// Three per triangle.
    pub indices: Vec<u16>,
    pub joint: Option<Joint>,
    pub keep_camera_outside: bool,
    pub beam_origin_mm: Option<[f64; 3]>,
}

/// The mesh for a `dish_model_for` id; anything unnamed borrows the
/// Standard's body.
pub fn mesh_for_model(model: &str) -> &'static DishModelMesh {
    let meshes = MESHES.get_or_init(load);
    meshes
        .get(model)
        .or_else(|| meshes.get("rev4Standard"))
        .expect("the Standard kit is bundled")
}

static MESHES: OnceLock<HashMap<String, DishModelMesh>> = OnceLock::new();

const MANIFEST: &str = include_str!("../assets/dish_models/manifest.json");

fn geometry(model: &str) -> &'static [u8] {
    macro_rules! bundled {
        ($($name:literal),* $(,)?) => {
            match model {
                $($name => include_bytes!(concat!("../assets/dish_models/", $name, ".bin")),)*
                _ => &[],
            }
        };
    }
    bundled!(
        "rev2Circular",
        "rev3Rectangular",
        "rev4Standard",
        "rev5Standard",
        "performanceGen1",
        "performanceGen2",
        "performanceGen3",
        "aviation",
        "mini1",
        "mini2",
    )
}

/// Decode the bundled meshes. The data ships with the binary and is checked
/// by the tests, so a malformed entry is a build defect, not a runtime case.
fn load() -> HashMap<String, DishModelMesh> {
    let headers: HashMap<String, Header> =
        serde_json::from_str(MANIFEST).expect("bundled dish manifest parses");
    headers
        .into_iter()
        .map(|(model, header)| {
            let bytes = geometry(&model);
            let split = header.vertex_count * 6;
            let positions = bytes[..split]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes(*pair))
                .collect();
            let indices = bytes[split..split + header.triangle_count * 6]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect();
            let mesh = DishModelMesh {
                scale: header.scale,
                long_axis_mm: header.long_axis_mm,
                parts: header.parts,
                positions,
                indices,
                joint: header.joint,
                keep_camera_outside: header.keep_camera_outside,
                beam_origin_mm: header.beam_origin_mm,
            };
            (model, mesh)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODELS: [&str; 10] = [
        "rev2Circular",
        "rev3Rectangular",
        "rev4Standard",
        "rev5Standard",
        "performanceGen1",
        "performanceGen2",
        "performanceGen3",
        "aviation",
        "mini1",
        "mini2",
    ];

    // SPEC-DISHMESH-001: every kit loads with indices covering its parts and
    // pointing at real vertices; unknown ids draw the Standard.
    #[test]
    fn every_kit_has_a_consistent_mesh() {
        for model in MODELS {
            let mesh = mesh_for_model(model);
            let triangles: usize = mesh.parts.iter().map(|part| part.1).sum();
            assert_eq!(mesh.indices.len(), triangles * 3, "{model}");
            let vertices = mesh.positions.len() / 3;
            assert!(
                mesh.indices
                    .iter()
                    .all(|index| usize::from(*index) < vertices),
                "{model}"
            );
            assert!(mesh.long_axis_mm > 0.0 && mesh.scale > 0.0);
        }
        assert!(std::ptr::eq(
            mesh_for_model("unknown"),
            mesh_for_model("rev4Standard")
        ));
        assert!(std::ptr::eq(
            mesh_for_model("rev9"),
            mesh_for_model("rev4Standard")
        ));
        assert!(mesh_for_model("rev2Circular").joint.is_some());
        assert!(mesh_for_model("aviation").keep_camera_outside);
        assert_eq!(
            mesh_for_model("aviation").beam_origin_mm,
            Some([0.0, 210.0, 504.0])
        );
    }
}
