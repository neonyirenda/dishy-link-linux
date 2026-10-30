//! The native sky renderer: one depth-tested GL pass over generated buffers.
//!
//! `dishy-sky` owns every vertex; this module owns the GPU objects, shaders,
//! and frame loop. It is deliberately small — the sky is a handful of static
//! buffers plus a point cloud that is re-uploaded when the survey changes.

use std::ffi::{CString, c_void};

use dishy_sky::satellite_mesh::{SATELLITE_FLOATS, SatelliteDetail, build_satellite};
use dishy_sky::{
    LIGHT, Mat4, SATELLITE_MODEL_SCALE, SatelliteLook, SkyCamera, Survey, TRAIL_FLOATS,
    build_compass, build_compass_labels, build_dish, build_dome_points, build_stars, build_terrain,
    build_trail_ribbon, drawn_position, look_at, multiply, perspective, satellite_basis,
    trail_points,
};
use glow::HasContext;

/// Clear the canvas to the same near-black the dashboard uses.
const BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const FOV_Y: f64 = std::f64::consts::FRAC_PI_3;
/// Amber is the sky view's word for "serving", in the legend and the callout.
pub const SERVING_COLOR: [f32; 3] = [1.0, 0.69, 0.25];
const SELECTED_COLOR: [f32; 3] = [0.45, 0.85, 1.0];
/// A tap within this many logical pixels picks the nearest satellite, the
/// same forgiving radius as `skyScene.ts`.
const PICK_RADIUS_PX: f64 = 26.0;

const MESH_VERTEX_SHADER: &str = r#"
#version 330 core
layout (location = 0) in vec3 aPos;
layout (location = 1) in vec3 aColor;
uniform mat4 uMvp;
out vec3 vColor;
void main() {
    gl_Position = uMvp * vec4(aPos, 1.0);
    vColor = aColor;
}
"#;

const MESH_FRAGMENT_SHADER: &str = r#"
#version 330 core
in vec3 vColor;
out vec4 color;
void main() {
    color = vec4(vColor, 1.0);
}
"#;

const POINT_VERTEX_SHADER: &str = r#"
#version 330 core
layout (location = 0) in vec3 aPos;
layout (location = 1) in vec3 aColor;
layout (location = 2) in float aSize;
uniform mat4 uMvp;
uniform float uScale;
out vec3 vColor;
void main() {
    gl_Position = uMvp * vec4(aPos, 1.0);
    gl_PointSize = aSize * uScale;
    vColor = aColor;
}
"#;

// The craft (spec 0184): one model drawn per instance in its own basis, lit as
// it turns. Instance attributes are the offset and the right/up/forward axes.
const SATELLITE_VERTEX_SHADER: &str = r#"
#version 330 core
layout (location = 0) in vec3 aPos;
layout (location = 1) in vec3 aNormal;
layout (location = 2) in vec3 aColor;
layout (location = 3) in vec3 iOffset;
layout (location = 4) in vec3 iRight;
layout (location = 5) in vec3 iUp;
layout (location = 6) in vec3 iForward;
uniform mat4 uMvp;
uniform float uScale;
uniform vec3 uLight;
out vec3 vColor;
void main() {
    mat3 basis = mat3(iRight, iUp, iForward);
    vec3 world = iOffset + basis * (aPos * uScale);
    float lambert = max(0.0, dot(normalize(basis * aNormal), uLight));
    vColor = aColor * (0.34 + 0.66 * lambert);
    gl_Position = uMvp * vec4(world, 1.0);
}
"#;

// The satellite trail (spec 0182): a camera-facing ribbon feathered across its
// width and faded along its length. White for every craft: sunlight off the
// hull, so the serving satellite is told apart by its dot alone.
const TRAIL_VERTEX_SHADER: &str = r#"
#version 330 core
layout (location = 0) in vec3 aPos;
layout (location = 1) in float aAcross;
layout (location = 2) in float aGlow;
uniform mat4 uMvp;
out float vAcross;
out float vGlow;
void main() {
    vAcross = aAcross;
    vGlow = aGlow;
    gl_Position = uMvp * vec4(aPos, 1.0);
}
"#;

const TRAIL_FRAGMENT_SHADER: &str = r#"
#version 330 core
in float vAcross;
in float vGlow;
out vec4 color;
void main() {
    // Full down the spine, feathering to nothing at the edges.
    float across = pow(clamp(1.0 - abs(vAcross), 0.0, 1.0), 1.6);
    // Additive blending multiplies rgb by alpha, so one factor is enough.
    color = vec4(1.0, 1.0, 1.0, across * vGlow);
}
"#;

const POINT_FRAGMENT_SHADER: &str = r#"
#version 330 core
in vec3 vColor;
out vec4 color;
void main() {
    vec2 offset = gl_PointCoord * 2.0 - 1.0;
    float distance = dot(offset, offset);
    if (distance > 1.0) discard;
    float alpha = smoothstep(1.0, 0.45, distance);
    color = vec4(vColor, alpha);
}
"#;

/// GTK4's GDK no longer exposes a proc-address getter, but the context it
/// creates is a normal libGL/libGLES context. Loading the library directly is
/// the one portable path that keeps glow as the GL binding rather than
/// hand-writing every entry point.
struct GlLoader {
    handles: Vec<*mut c_void>,
}

// The handles are process-wide library handles and are only ever read after
// construction; the frame loop runs on the GTK main thread.
unsafe impl Send for GlLoader {}

impl GlLoader {
    fn new() -> Self {
        let mut handles = Vec::new();
        for name in ["libGL.so.1", "libGLESv2.so.2", "libEGL.so.1"] {
            let Ok(name) = CString::new(name) else {
                continue;
            };
            // SAFETY: `name` is a valid NUL-terminated string.
            let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
            if !handle.is_null() {
                handles.push(handle);
            }
        }
        Self { handles }
    }

    unsafe fn load(&self, name: &str) -> *const c_void {
        let Ok(name) = CString::new(name) else {
            return std::ptr::null();
        };
        for handle in &self.handles {
            // SAFETY: the handle came from dlopen and the name is NUL-terminated.
            let symbol = unsafe { libc::dlsym(*handle, name.as_ptr()) };
            if !symbol.is_null() {
                return symbol;
            }
        }
        // SAFETY: RTLD_DEFAULT asks the loader for a symbol already in scope.
        unsafe { libc::dlsym(std::ptr::null_mut(), name.as_ptr()) }
    }
}

impl Drop for GlLoader {
    fn drop(&mut self) {
        for handle in self.handles.drain(..) {
            // SAFETY: each handle was returned by dlopen exactly once.
            unsafe {
                libc::dlclose(handle);
            }
        }
    }
}

struct Programs {
    mesh: glow::Program,
    point: glow::Program,
    mesh_mvp: glow::UniformLocation,
    point_mvp: glow::UniformLocation,
    point_scale: glow::UniformLocation,
    trail: glow::Program,
    trail_mvp: glow::UniformLocation,
    craft: glow::Program,
    craft_mvp: glow::UniformLocation,
    craft_scale: glow::UniformLocation,
    craft_light: glow::UniformLocation,
}

pub struct SkyRenderer {
    loader: GlLoader,
    gl: glow::Context,
    programs: Programs,
    vao: glow::VertexArray,
    dome_buffer: glow::Buffer,
    star_buffer: glow::Buffer,
    mesh_buffer: glow::Buffer,
    dish_buffer: glow::Buffer,
    satellite_buffer: glow::Buffer,
    trail_buffer: glow::Buffer,
    craft_buffer: glow::Buffer,
    craft_vertices: i32,
    instance_buffer: glow::Buffer,
    dome_vertices: i32,
    star_vertices: i32,
    mesh_vertices: i32,
    dish_vertices: i32,
    satellite_vertices: i32,
    camera: SkyCamera,
    dome_visible: bool,
    trim_unmapped: bool,
    last_frame_ms: f64,
    last_dish_radius: f32,
    current_survey: Option<Survey>,
    pending_survey: Option<Survey>,
    satellites: SatelliteLayer,
    pending_satellites: Option<SatelliteLayer>,
    last_mvp: Option<Mat4>,
    last_size: (i32, i32),
    /// Seconds since the satellites were sampled, as of the last frame.
    last_since_sample_s: f64,
}

/// What the constellation layer draws: the look angles, plus which one is
/// serving and which one the user tapped.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SatelliteLayer {
    pub satellites: Vec<SatelliteLook>,
    pub serving: Option<String>,
    pub selected: Option<String>,
    /// The sample time on GLib's monotonic clock (spec 0182).
    pub sampled_at_us: i64,
}

impl SkyRenderer {
    pub fn new() -> Result<Self, String> {
        let loader = GlLoader::new();
        if loader.handles.is_empty() {
            return Err("no OpenGL library could be loaded".into());
        }
        // SAFETY: the caller guarantees a current GL context of at least 3.3.
        let gl = unsafe { glow::Context::from_loader_function(|name| loader.load(name)) };
        // SAFETY: every call below runs with the GL context current.
        unsafe {
            let version = gl.get_parameter_string(glow::VERSION);
            if version.is_empty() {
                return Err("the GL context did not report a version".into());
            }
            let mesh_program = link_program(&gl, MESH_VERTEX_SHADER, MESH_FRAGMENT_SHADER)?;
            let point_program = link_program(&gl, POINT_VERTEX_SHADER, POINT_FRAGMENT_SHADER)?;
            let mesh_mvp = gl
                .get_uniform_location(mesh_program, "uMvp")
                .ok_or("mesh shader has no uMvp")?;
            let point_mvp = gl
                .get_uniform_location(point_program, "uMvp")
                .ok_or("point shader has no uMvp")?;
            let point_scale = gl
                .get_uniform_location(point_program, "uScale")
                .ok_or("point shader has no uScale")?;
            let trail_program = link_program(&gl, TRAIL_VERTEX_SHADER, TRAIL_FRAGMENT_SHADER)?;
            let trail_mvp = gl
                .get_uniform_location(trail_program, "uMvp")
                .ok_or("trail shader has no uMvp")?;
            let craft_program = link_program(&gl, SATELLITE_VERTEX_SHADER, MESH_FRAGMENT_SHADER)?;
            let craft_uniform = |name: &str| {
                gl.get_uniform_location(craft_program, name)
                    .ok_or_else(|| format!("craft shader has no {name}"))
            };
            let craft_mvp = craft_uniform("uMvp")?;
            let craft_scale = craft_uniform("uScale")?;
            let craft_light = craft_uniform("uLight")?;
            let programs = Programs {
                mesh: mesh_program,
                point: point_program,
                mesh_mvp,
                point_mvp,
                point_scale,
                trail: trail_program,
                trail_mvp,
                craft: craft_program,
                craft_mvp,
                craft_scale,
                craft_light,
            };
            let vao = gl
                .create_vertex_array()
                .map_err(|error| error.to_string())?;
            gl.bind_vertex_array(Some(vao));
            let make_buffer = || gl.create_buffer().map_err(|error| error.to_string());
            let dome_buffer = make_buffer()?;
            let star_buffer = make_buffer()?;
            let mesh_buffer = make_buffer()?;
            let dish_buffer = make_buffer()?;
            let satellite_buffer = make_buffer()?;
            let trail_buffer = make_buffer()?;
            let craft_buffer = make_buffer()?;
            let instance_buffer = make_buffer()?;
            // The sky draws the distant detail, as Electron does.
            let craft = build_satellite(SatelliteDetail::Distant);
            upload(&gl, craft_buffer, &craft);

            let stars = stars_as_points();
            let star_vertices = (stars.len() / 7) as i32;
            upload(&gl, star_buffer, &stars);
            let mut static_mesh = build_terrain();
            static_mesh.extend_from_slice(&build_compass());
            static_mesh.extend_from_slice(&build_compass_labels());
            upload(&gl, mesh_buffer, &static_mesh);
            upload(&gl, satellite_buffer, &[]);

            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
            gl.enable(glow::PROGRAM_POINT_SIZE);
            gl.clear_color(BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], BACKGROUND[3]);

            Ok(Self {
                loader,
                gl,
                programs,
                vao,
                dome_buffer,
                star_buffer,
                mesh_buffer,
                dish_buffer,
                satellite_buffer,
                trail_buffer,
                craft_buffer,
                craft_vertices: (craft.len() / SATELLITE_FLOATS) as i32,
                instance_buffer,
                dome_vertices: 0,
                star_vertices,
                mesh_vertices: (static_mesh.len() / 6) as i32,
                dish_vertices: 0,
                satellite_vertices: 0,
                camera: SkyCamera::new(),
                dome_visible: true,
                trim_unmapped: false,
                last_frame_ms: 0.0,
                last_dish_radius: 0.0,
                current_survey: None,
                pending_survey: None,
                satellites: SatelliteLayer::default(),
                pending_satellites: None,
                last_mvp: None,
                last_size: (0, 0),
                last_since_sample_s: 0.0,
            })
        }
    }

    pub fn camera_mut(&mut self) -> &mut SkyCamera {
        &mut self.camera
    }

    pub fn is_rotating(&self) -> bool {
        self.camera.is_rotating()
    }

    pub fn is_dome_visible(&self) -> bool {
        self.dome_visible
    }

    pub fn toggle_dome(&mut self) -> bool {
        self.dome_visible = !self.dome_visible;
        self.dome_visible
    }

    pub fn toggle_rotation(&mut self) -> bool {
        self.camera.toggle_rotation()
    }

    pub fn reset_view(&mut self) {
        self.camera.reset_view();
    }

    pub fn set_trim_unmapped(&mut self, trim: bool) {
        if self.trim_unmapped == trim {
            return;
        }
        self.trim_unmapped = trim;
        self.pending_survey = self.current_survey.clone();
    }

    /// Queue a new survey. The upload happens at the next render, when the GL
    /// context is guaranteed current, so a status poll never touches GL from a
    /// button or channel callback.
    pub fn set_survey(&mut self, survey: Survey) {
        self.pending_survey = Some(survey);
    }

    /// Queue the constellation layer. The upload happens at the next render.
    pub fn set_satellites(&mut self, layer: SatelliteLayer) {
        self.pending_satellites = Some(layer);
    }

    /// Where a satellite is drawn in the last frame, in logical pixels from
    /// the canvas's top-left. `None` before the first frame or when the point
    /// is behind the camera.
    pub fn project_satellite(&self, look: &SatelliteLook) -> Option<(f64, f64)> {
        let mvp = self.last_mvp.as_ref()?;
        let (width, height) = self.last_size;
        project_point(
            mvp,
            drawn_position(look, self.last_since_sample_s),
            width,
            height,
        )
    }

    /// The satellite nearest a tap, if one is within the pick radius.
    pub fn pick(&self, x: f64, y: f64) -> Option<String> {
        let mut best: Option<(f64, &SatelliteLook)> = None;
        for look in &self.satellites.satellites {
            let Some((screen_x, screen_y)) = self.project_satellite(look) else {
                continue;
            };
            let distance = (screen_x - x).hypot(screen_y - y);
            if distance <= PICK_RADIUS_PX && best.is_none_or(|(nearest, _)| distance < nearest) {
                best = Some((distance, look));
            }
        }
        best.map(|(_, look)| look.name.clone())
    }

    fn apply_pending_survey(&mut self) {
        let Some(survey) = self.pending_survey.take() else {
            return;
        };
        let dome = dome_as_points(&survey, self.trim_unmapped);
        let dish = build_dish(&survey, 1.0);
        upload(&self.gl, self.dome_buffer, &dome);
        upload(&self.gl, self.dish_buffer, &dish.data);
        self.dome_vertices = (dome.len() / 7) as i32;
        self.dish_vertices = (dish.data.len() / 6) as i32;
        if (dish.radius - self.last_dish_radius).abs() > 0.001 {
            // Electron's DISH_CLEARANCE; zero (no hold) for kits without an inside.
            self.camera.set_min_distance(f64::from(dish.radius) * 1.1);
            self.last_dish_radius = dish.radius;
        }
        self.current_survey = Some(survey);
    }

    fn apply_pending_satellites(&mut self) {
        let Some(layer) = self.pending_satellites.take() else {
            return;
        };
        self.satellites = layer;
    }

    /// The satellites at their drawn positions and their trails, rebuilt every
    /// frame from the sample so nothing stale is ever held (spec 0182).
    fn upload_moving_satellites(&mut self, now_ms: f64, eye: [f32; 3]) -> i32 {
        let since = (now_ms * 1_000.0 - self.satellites.sampled_at_us as f64) / 1e6;
        self.last_since_sample_s = since;
        upload(
            &self.gl,
            self.instance_buffer,
            &satellite_instances(&self.satellites, since),
        );
        let points = satellites_as_points(&self.satellites, since);
        upload(&self.gl, self.satellite_buffer, &points);
        self.satellite_vertices = (points.len() / 7) as i32;
        let trails: Vec<Vec<[f32; 3]>> = self
            .satellites
            .satellites
            .iter()
            .map(|look| trail_points(look, since))
            .collect();
        let ribbon = build_trail_ribbon(&trails, eye);
        upload(&self.gl, self.trail_buffer, &ribbon);
        (ribbon.len() / TRAIL_FLOATS) as i32
    }

    pub fn render(&mut self, width: i32, height: i32, scale: f32, now_ms: f64) {
        if width <= 0 || height <= 0 {
            return;
        }
        self.apply_pending_survey();
        self.apply_pending_satellites();
        let dt = if self.last_frame_ms == 0.0 {
            0.0
        } else {
            ((now_ms - self.last_frame_ms) / 1_000.0).clamp(0.0, 0.1)
        };
        self.last_frame_ms = now_ms;
        let view = self.camera.view(now_ms, dt);
        let projection = perspective(FOV_Y, f64::from(width) / f64::from(height), 0.05, 120.0);
        let view_matrix: Mat4 = look_at(view.eye, view.target, [0.0, 1.0, 0.0]);
        let mvp = multiply(&projection, &view_matrix);
        self.last_mvp = Some(mvp);
        self.last_size = (width, height);
        let trail_vertices =
            self.upload_moving_satellites(now_ms, view.eye.map(|value| value as f32));
        let physical_width = (width as f32 * scale) as i32;
        let physical_height = (height as f32 * scale) as i32;

        // SAFETY: the context is current.
        unsafe {
            self.gl.viewport(0, 0, physical_width, physical_height);
            self.gl
                .clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            self.gl.bind_vertex_array(Some(self.vao));

            self.draw_points(self.star_buffer, self.star_vertices, &mvp, scale);
            self.draw_mesh(self.mesh_buffer, self.mesh_vertices, &mvp);
            self.draw_mesh(self.dish_buffer, self.dish_vertices, &mvp);
            if self.dome_visible {
                self.draw_points(self.dome_buffer, self.dome_vertices, &mvp, scale);
            }
            self.draw_trails(trail_vertices, &mvp);
            self.draw_crafts(&mvp);
            self.draw_points(self.satellite_buffer, self.satellite_vertices, &mvp, scale);
            self.gl.bind_vertex_array(None);
        }
    }

    unsafe fn draw_points(&self, buffer: glow::Buffer, count: i32, mvp: &Mat4, scale: f32) {
        if count == 0 {
            return;
        }
        unsafe {
            self.gl.use_program(Some(self.programs.point));
            self.gl
                .uniform_matrix_4_f32_slice(Some(&self.programs.point_mvp), false, mvp);
            self.gl
                .uniform_1_f32(Some(&self.programs.point_scale), scale.max(1.0));
            self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            self.gl.enable_vertex_attrib_array(0);
            self.gl
                .vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 28, 0);
            self.gl.enable_vertex_attrib_array(1);
            self.gl
                .vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, 28, 12);
            self.gl.enable_vertex_attrib_array(2);
            self.gl
                .vertex_attrib_pointer_f32(2, 1, glow::FLOAT, false, 28, 24);
            self.gl.draw_arrays(glow::POINTS, 0, count);
        }
    }

    /// One model for the whole constellation, instanced in each craft's basis.
    unsafe fn draw_crafts(&self, mvp: &Mat4) {
        let count = self.satellites.satellites.len() as i32;
        if count == 0 || self.craft_vertices == 0 {
            return;
        }
        let vertex_stride = (SATELLITE_FLOATS * 4) as i32;
        let instance_stride = (INSTANCE_FLOATS * 4) as i32;
        unsafe {
            self.gl.use_program(Some(self.programs.craft));
            self.gl
                .uniform_matrix_4_f32_slice(Some(&self.programs.craft_mvp), false, mvp);
            self.gl.uniform_1_f32(
                Some(&self.programs.craft_scale),
                SATELLITE_MODEL_SCALE as f32,
            );
            self.gl.uniform_3_f32(
                Some(&self.programs.craft_light),
                LIGHT[0] as f32,
                LIGHT[1] as f32,
                LIGHT[2] as f32,
            );
            self.gl
                .bind_buffer(glow::ARRAY_BUFFER, Some(self.craft_buffer));
            for (location, offset) in [(0, 0), (1, 12), (2, 24)] {
                self.gl.enable_vertex_attrib_array(location);
                self.gl.vertex_attrib_pointer_f32(
                    location,
                    3,
                    glow::FLOAT,
                    false,
                    vertex_stride,
                    offset,
                );
            }
            self.gl
                .bind_buffer(glow::ARRAY_BUFFER, Some(self.instance_buffer));
            for (location, offset) in [(3, 0), (4, 12), (5, 24), (6, 36)] {
                self.gl.enable_vertex_attrib_array(location);
                self.gl.vertex_attrib_pointer_f32(
                    location,
                    3,
                    glow::FLOAT,
                    false,
                    instance_stride,
                    offset,
                );
                self.gl.vertex_attrib_divisor(location, 1);
            }
            self.gl
                .draw_arrays_instanced(glow::TRIANGLES, 0, self.craft_vertices, count);
            // Leave the instance attributes off so later passes do not read them.
            for location in 3..=6 {
                self.gl.vertex_attrib_divisor(location, 0);
                self.gl.disable_vertex_attrib_array(location);
            }
        }
    }

    /// Additive, so the trails glow rather than paint, and without depth
    /// writes, so they never carve into the sky behind them.
    unsafe fn draw_trails(&self, count: i32, mvp: &Mat4) {
        if count == 0 {
            return;
        }
        let stride = (TRAIL_FLOATS * 4) as i32;
        unsafe {
            self.gl.use_program(Some(self.programs.trail));
            self.gl
                .uniform_matrix_4_f32_slice(Some(&self.programs.trail_mvp), false, mvp);
            self.gl
                .bind_buffer(glow::ARRAY_BUFFER, Some(self.trail_buffer));
            self.gl.enable_vertex_attrib_array(0);
            self.gl
                .vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
            self.gl.enable_vertex_attrib_array(1);
            self.gl
                .vertex_attrib_pointer_f32(1, 1, glow::FLOAT, false, stride, 12);
            self.gl.enable_vertex_attrib_array(2);
            self.gl
                .vertex_attrib_pointer_f32(2, 1, glow::FLOAT, false, stride, 16);
            self.gl.blend_func(glow::SRC_ALPHA, glow::ONE);
            self.gl.depth_mask(false);
            self.gl.draw_arrays(glow::TRIANGLES, 0, count);
            self.gl.depth_mask(true);
            self.gl
                .blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
        }
    }

    unsafe fn draw_mesh(&self, buffer: glow::Buffer, count: i32, mvp: &Mat4) {
        if count == 0 {
            return;
        }
        unsafe {
            self.gl.use_program(Some(self.programs.mesh));
            self.gl
                .uniform_matrix_4_f32_slice(Some(&self.programs.mesh_mvp), false, mvp);
            self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            self.gl.enable_vertex_attrib_array(0);
            self.gl
                .vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 24, 0);
            self.gl.enable_vertex_attrib_array(1);
            self.gl
                .vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, 24, 12);
            self.gl.disable_vertex_attrib_array(2);
            self.gl.draw_arrays(glow::TRIANGLES, 0, count);
        }
    }
}

/// Kept separate so the loader outlives the GL context by declaration order.
impl Drop for SkyRenderer {
    fn drop(&mut self) {
        // SAFETY: the page drops the renderer while its GLArea is unrealized and
        // has made the context current.
        unsafe {
            for program in [
                self.programs.mesh,
                self.programs.point,
                self.programs.trail,
                self.programs.craft,
            ] {
                self.gl.delete_program(program);
            }
            for buffer in [
                self.dome_buffer,
                self.star_buffer,
                self.mesh_buffer,
                self.dish_buffer,
                self.satellite_buffer,
                self.trail_buffer,
                self.craft_buffer,
                self.instance_buffer,
            ] {
                self.gl.delete_buffer(buffer);
            }
            self.gl.delete_vertex_array(self.vao);
        }
        let _ = &self.loader;
    }
}

fn upload(gl: &glow::Context, buffer: glow::Buffer, data: &[f32]) {
    // SAFETY: the context is current and the slice is valid for its byte length.
    unsafe {
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
        let bytes = std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 4);
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
    }
}

fn stars_as_points() -> Vec<f32> {
    let stars = build_stars();
    let mut out = Vec::with_capacity(stars.len() / 4 * 7);
    let (stars, _) = stars.as_chunks::<4>();
    for star in stars {
        out.extend_from_slice(&[star[0], star[1], star[2], 0.72, 0.78, 0.9, star[3]]);
    }
    out
}

fn dome_as_points(survey: &Survey, trim: bool) -> Vec<f32> {
    let points = build_dome_points(survey, trim);
    let mut out = Vec::with_capacity(points.len() / 4 * 7);
    let (points, _) = points.as_chunks::<4>();
    for point in points {
        let kind = dishy_sky::DomePointKind::from_code(point[3] as u8);
        let color = kind.color();
        out.extend_from_slice(&[
            point[0], point[1], point[2], color[0], color[1], color[2], 3.2,
        ]);
    }
    out
}

/// Floats per craft instance: offset, then the right, up, and forward axes.
const INSTANCE_FLOATS: usize = 12;

fn satellite_instances(layer: &SatelliteLayer, since_sample_s: f64) -> Vec<f32> {
    let mut out = Vec::with_capacity(layer.satellites.len() * INSTANCE_FLOATS);
    for look in &layer.satellites {
        let position = drawn_position(look, since_sample_s);
        out.extend_from_slice(&position);
        for axis in satellite_basis(look, position) {
            out.extend_from_slice(&axis);
        }
    }
    out
}

/// The serving and selected markers. Every other craft is its model alone.
fn satellites_as_points(layer: &SatelliteLayer, since_sample_s: f64) -> Vec<f32> {
    let mut out = Vec::new();
    for look in &layer.satellites {
        let [x, y, z] = drawn_position(look, since_sample_s);
        let (color, size) = if layer.serving.as_deref() == Some(look.name.as_str()) {
            (SERVING_COLOR, 11.0)
        } else if layer.selected.as_deref() == Some(look.name.as_str()) {
            (SELECTED_COLOR, 9.0)
        } else {
            continue;
        };
        out.extend_from_slice(&[x, y, z, color[0], color[1], color[2], size]);
    }
    out
}

/// Clip space to logical pixels, top-left origin.
fn project_point(mvp: &Mat4, point: [f32; 3], width: i32, height: i32) -> Option<(f64, f64)> {
    let [x, y, z] = point.map(f64::from);
    let m = |index: usize| f64::from(mvp[index]);
    let column = |row: usize| m(row) * x + m(4 + row) * y + m(8 + row) * z + m(12 + row);
    let w = column(3);
    if w <= 1e-6 {
        return None;
    }
    let ndc_x = column(0) / w;
    let ndc_y = column(1) / w;
    Some((
        (ndc_x + 1.0) * 0.5 * f64::from(width),
        (1.0 - ndc_y) * 0.5 * f64::from(height),
    ))
}

fn link_program(
    gl: &glow::Context,
    vertex_source: &str,
    fragment_source: &str,
) -> Result<glow::Program, String> {
    // SAFETY: the context is current; every GL object is deleted on failure.
    unsafe {
        let program = gl.create_program().map_err(|error| error.to_string())?;
        let mut shaders = Vec::new();
        for (kind, source) in [
            (glow::VERTEX_SHADER, vertex_source),
            (glow::FRAGMENT_SHADER, fragment_source),
        ] {
            let shader = gl.create_shader(kind).map_err(|error| error.to_string())?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let log = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                for shader in shaders {
                    gl.delete_shader(shader);
                }
                gl.delete_program(program);
                return Err(format!("sky shader failed to compile: {log}"));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            for shader in shaders {
                gl.delete_shader(shader);
            }
            gl.delete_program(program);
            return Err(format!("sky shader failed to link: {log}"));
        }
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        Ok(program)
    }
}

/// Test-only construction of the vertex converters, so the buffer layout is
/// checked without a GPU.
#[cfg(test)]
pub fn point_buffer_for_test(survey: &Survey) -> Vec<f32> {
    dome_as_points(survey, false)
}

/// A dish's mesh is uploaded unchanged; exposed so a test can assert the layout
/// matches the renderer's six-float stride.
#[cfg(test)]
pub fn mesh_buffer_for_test(dish: &dishy_sky::DishMesh) -> &[f32] {
    &dish.data
}

/// Shared between the loader and the shader tests to keep the version honest.
pub fn required_version() -> (i32, i32) {
    (3, 3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishy_sky::DomePointKind;

    fn survey() -> Survey {
        Survey {
            grid_size: 5,
            max_theta_deg: 80.0,
            kinds: vec![DomePointKind::Clear.code(); 25],
            boresight_azimuth_deg: 0.0,
            boresight_elevation_deg: 90.0,
            frame: dishy_sky::ObstructionFrame::Earth,
            dish_model: "rev4Standard".into(),
        }
    }

    // SPEC-SATMODEL-004: one instance per craft at its drawn position and
    // basis; dots remain only as the serving and selected markers.
    #[test]
    fn crafts_are_instanced_and_only_markers_are_dots() {
        let look = |name: &str| SatelliteLook {
            name: name.into(),
            azimuth_deg: 30.0,
            elevation_deg: 50.0,
            range_km: 700.0,
            altitude_km: 550.0,
            speed_km_s: 7.6,
            enu_km: [0.0; 3],
            enu_velocity_km_s: [0.0; 3],
            attitude: None,
        };
        let layer = SatelliteLayer {
            satellites: vec![look("A"), look("B"), look("C")],
            serving: Some("A".into()),
            selected: Some("C".into()),
            sampled_at_us: 0,
        };
        let instances = satellite_instances(&layer, 0.0);
        assert_eq!(instances.len(), 3 * INSTANCE_FLOATS);
        let position = dishy_sky::drawn_position(&layer.satellites[1], 0.0);
        assert_eq!(&instances[INSTANCE_FLOATS..INSTANCE_FLOATS + 3], &position);
        let basis = dishy_sky::satellite_basis(&layer.satellites[1], position);
        assert_eq!(
            &instances[INSTANCE_FLOATS + 3..INSTANCE_FLOATS + 6],
            &basis[0]
        );
        let markers = satellites_as_points(&layer, 0.0);
        assert_eq!(markers.len(), 2 * 7, "serving and selected only");
        assert_eq!(&markers[3..6], &SERVING_COLOR);
        assert_eq!(&markers[7 + 3..7 + 6], &SELECTED_COLOR);
    }

    // SPEC-SKY-005: the renderer's attribute strides are what the generated
    // buffers actually contain — seven floats for points, six for triangles.
    #[test]
    fn generated_buffers_match_the_renderers_strides() {
        let points = point_buffer_for_test(&survey());
        assert!(!points.is_empty());
        assert!(points.len().is_multiple_of(7));
        assert!(points.iter().all(|value| value.is_finite()));
        let (points, _) = points.as_chunks::<7>();
        assert!(points.iter().all(|point| point[6] > 0.0));

        let dish = build_dish(&survey(), 1.0);
        let mesh: &[f32] = mesh_buffer_for_test(&dish);
        assert!(mesh.len().is_multiple_of(6));
        assert!(mesh.iter().all(|value| value.is_finite()));
    }
}
