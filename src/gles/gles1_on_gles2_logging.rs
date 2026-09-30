use crate::gles::gles2_raw as gl;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static INITIALIZATION_LOGGED: AtomicBool = AtomicBool::new(false);
static GL_STATE_SUMMARY_LOGGED: AtomicBool = AtomicBool::new(false);
static INSTRUMENTATION_BANNER_LOGGED: AtomicBool = AtomicBool::new(false);
static ENV_LOGGING_ENABLED: OnceLock<bool> = OnceLock::new();

pub(crate) fn enabled() -> bool {
    crate::gles::translator_tracing_enabled()
        || *ENV_LOGGING_ENABLED
            .get_or_init(|| std::env::var_os("TOUCHHLE_GLES1_GLES2_LOG").is_some())
}

pub struct GLES1to2Logger {
    operation_id: u64,
    operation_name: &'static str,
    started_at: Instant,
    context: &'static str,
}

impl GLES1to2Logger {
    pub(crate) fn operation_id(&self) -> u64 {
        self.operation_id
    }

    pub fn new(operation_name: &'static str, context: &'static str) -> Self {
        Self {
            operation_id: crate::gles::next_gl_call_id(),
            operation_name,
            started_at: Instant::now(),
            context,
        }
    }

    pub fn log_matrix(&self, label: &str, matrix: &[f32; 16], before_fix: bool) {
        if !enabled() {
            return;
        }
        let phase = if before_fix { "before" } else { "after" };
        log!(
            "[GLES1→GLES2 MATRIX] op={} name={} context={} phase={} label={}",
            self.operation_id,
            self.operation_name,
            self.context,
            phase,
            label
        );
        for row in 0..4 {
            log!(
                "[GLES1→GLES2 MATRIX] op={} row={} values=({:.6},{:.6},{:.6},{:.6})",
                self.operation_id,
                row,
                matrix[row],
                matrix[row + 4],
                matrix[row + 8],
                matrix[row + 12]
            );
        }
        let x_scale = column_length(matrix, 0);
        let y_scale = column_length(matrix, 1);
        let z_scale = column_length(matrix, 2);
        let det = determinant(matrix);
        let largest_scale = x_scale.max(y_scale).max(z_scale);
        let scale_tolerance = (largest_scale * 0.01).max(0.0001);
        let uniform_scale = (x_scale - y_scale).abs() <= scale_tolerance
            && (y_scale - z_scale).abs() <= scale_tolerance;
        let orientation = if det < -0.000001 {
            "mirrored"
        } else if det.abs() <= 0.000001 {
            "singular"
        } else {
            "normal"
        };
        log!(
            "[GLES1→GLES2 MATRIX_PROPERTIES] op={} determinant={:.6} trace={:.6} scale=({:.6},{:.6},{:.6}) translation=({:.6},{:.6},{:.6})",
            self.operation_id,
            det,
            matrix[0] + matrix[5] + matrix[10] + matrix[15],
            x_scale,
            y_scale,
            z_scale,
            matrix[12],
            matrix[13],
            matrix[14]
        );
        log!(
            "[GLES1→GLES2 MATRIX_VALIDATION] op={} uniform_scale={} orientation={} finite={} affine_bottom_row={:.6},{:.6},{:.6},{:.6}",
            self.operation_id,
            uniform_scale,
            orientation,
            matrix.iter().all(|value| value.is_finite()),
            matrix[3],
            matrix[7],
            matrix[11],
            matrix[15]
        );
    }

    pub fn log_vertex_batch(&self, label: &str, vertices: &[[f32; 3]], sample_size: usize) {
        if !enabled() {
            return;
        }
        log!(
            "[GLES1→GLES2 VERTICES] op={} label={} total={} sample_size={}",
            self.operation_id,
            label,
            vertices.len(),
            sample_size.min(vertices.len())
        );
        for (index, vertex) in vertices.iter().take(sample_size).enumerate() {
            log!(
                "[GLES1→GLES2 VERTEX] op={} index={} value=({:.6},{:.6},{:.6})",
                self.operation_id,
                index,
                vertex[0],
                vertex[1],
                vertex[2]
            );
        }
    }

    pub fn log_vertex_transformation(&self, original: [f32; 3], transformed: [f32; 4]) {
        if !enabled() {
            return;
        }
        log!(
            "[GLES1→GLES2 VERTEX_TRANSFORM] op={} original=({:.6},{:.6},{:.6}) clip=({:.6},{:.6},{:.6},{:.6})",
            self.operation_id,
            original[0],
            original[1],
            original[2],
            transformed[0],
            transformed[1],
            transformed[2],
            transformed[3]
        );
        if transformed[3] != 0.0 {
            log!(
                "[GLES1→GLES2 NDC] op={} value=({:.6},{:.6},{:.6})",
                self.operation_id,
                transformed[0] / transformed[3],
                transformed[1] / transformed[3],
                transformed[2] / transformed[3]
            );
        }
    }

    pub fn log_rotation_operation(
        &self,
        angle: f32,
        axis: (f32, f32, f32),
        axis_after_fix: (f32, f32, f32),
    ) {
        if !enabled() {
            return;
        }
        let turns = (angle / 90.0).round();
        let is_right_angle = (angle - turns * 90.0).abs() < 0.01;
        log!(
            "[GLES1→GLES2 ROTATION] op={} angle={:.6} axis=({:.6},{:.6},{:.6}) axis_after_fix=({:.6},{:.6},{:.6}) right_angle={}",
            self.operation_id,
            angle,
            axis.0,
            axis.1,
            axis.2,
            axis_after_fix.0,
            axis_after_fix.1,
            axis_after_fix.2,
            is_right_angle
        );
    }

    pub fn log_viewport(
        &self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        after_fix: Option<(i32, i32, u32, u32)>,
    ) {
        if !enabled() {
            return;
        }
        let before_aspect = aspect(width, height);
        log!(
            "[GLES1→GLES2 VIEWPORT] op={} before=({},{},{},{}) before_aspect={:.6}",
            self.operation_id,
            x,
            y,
            width,
            height,
            before_aspect
        );
        if let Some((fixed_x, fixed_y, fixed_width, fixed_height)) = after_fix {
            let fixed_aspect = aspect(fixed_width, fixed_height);
            let valid = fixed_x >= 0
                && fixed_y >= 0
                && fixed_width > 0
                && fixed_height > 0
                && fixed_width <= i32::MAX as u32
                && fixed_height <= i32::MAX as u32;
            log!(
                "[GLES1→GLES2 VIEWPORT] op={} after=({},{},{},{}) after_aspect={:.6}",
                self.operation_id,
                fixed_x,
                fixed_y,
                fixed_width,
                fixed_height,
                fixed_aspect
            );
            log!(
                "[GLES1→GLES2 VIEWPORT_VALIDATION] op={} positive_bounds={} aspect_finite={} aspect={:.6}",
                self.operation_id,
                valid,
                fixed_aspect.is_finite(),
                fixed_aspect
            );
        }
    }

    pub fn log_projection(
        &self,
        operation: &str,
        before: (f64, f64, f64, f64, f64, f64),
        after: Option<(f64, f64, f64, f64, f64, f64)>,
    ) {
        if !enabled() {
            return;
        }
        log!(
            "[GLES1→GLES2 PROJECTION] op={} kind={} before=({:.6},{:.6},{:.6},{:.6},{:.6},{:.6})",
            self.operation_id,
            operation,
            before.0,
            before.1,
            before.2,
            before.3,
            before.4,
            before.5
        );
        if let Some(after) = after {
            log!(
                "[GLES1→GLES2 PROJECTION] op={} kind={} after=({:.6},{:.6},{:.6},{:.6},{:.6},{:.6})",
                self.operation_id,
                operation,
                after.0,
                after.1,
                after.2,
                after.3,
                after.4,
                after.5
            );
        }
    }

    pub fn log_texture_coordinates(&self, unit: usize, coordinates: [f32; 4]) {
        if !enabled() {
            return;
        }
        log!(
            "[GLES1→GLES2 TEXCOORD] op={} unit={} value=({:.6},{:.6},{:.6},{:.6})",
            self.operation_id,
            unit,
            coordinates[0],
            coordinates[1],
            coordinates[2],
            coordinates[3]
        );
    }

    pub fn finish(&self) {
        if enabled() {
            log!(
                "[GLES1→GLES2 OPERATION] op={} name={} elapsed_us={}",
                self.operation_id,
                self.operation_name,
                self.started_at.elapsed().as_micros()
            );
        }
    }

    pub fn log_stage(&self, stage: &str, details: &str) {
        if enabled() {
            log!(
                "[GLES1→GLES2 PIPELINE] op={} stage={} {}",
                self.operation_id,
                stage,
                details
            );
        }
    }

    pub fn log_error(&self, error: u32) {
        if enabled() {
            let error_name = match error {
                gl::NO_ERROR => "GL_NO_ERROR",
                gl::INVALID_ENUM => "GL_INVALID_ENUM",
                gl::INVALID_VALUE => "GL_INVALID_VALUE",
                gl::INVALID_OPERATION => "GL_INVALID_OPERATION",
                gl::OUT_OF_MEMORY => "GL_OUT_OF_MEMORY",
                _ => "UNKNOWN_GL_ERROR",
            };
            log!(
                "[GLES1→GLES2 PIPELINE] op={} name={} context={} gl_error=0x{:x} ({})",
                self.operation_id,
                self.operation_name,
                self.context,
                error,
                error_name
            );
        }
    }
}

pub fn log_initialization(rotation_mode: &str) {
    if !enabled() || INITIALIZATION_LOGGED.swap(true, Ordering::Relaxed) {
        return;
    }
    log!(
        "[GLES1→GLES2 INITIALIZED] rotation_mode={} logging=matrix,vertices,transformations,texture_coordinates,viewport,scissor,texture_uploads,projection",
        rotation_mode
    );
}

pub fn instrument_rendering_pipeline() {
    if enabled() && !INSTRUMENTATION_BANNER_LOGGED.swap(true, Ordering::Relaxed) {
        log!("[RENDERING PIPELINE INSTRUMENTATION] viewport, scissor, projection matrices, vertex samples, coordinate transformations, and texture uploads are enabled");
    }
}

fn aspect(width: u32, height: u32) -> f32 {
    if height == 0 {
        0.0
    } else {
        width as f32 / height as f32
    }
}

fn column_length(matrix: &[f32; 16], column: usize) -> f32 {
    let offset = column * 4;
    (matrix[offset].powi(2) + matrix[offset + 1].powi(2) + matrix[offset + 2].powi(2)).sqrt()
}

fn determinant(matrix: &[f32; 16]) -> f32 {
    let m = |row: usize, column: usize| matrix[column * 4 + row];
    m(0, 0)
        * (m(1, 1) * (m(2, 2) * m(3, 3) - m(2, 3) * m(3, 2))
            - m(1, 2) * (m(2, 1) * m(3, 3) - m(2, 3) * m(3, 1))
            + m(1, 3) * (m(2, 1) * m(3, 2) - m(2, 2) * m(3, 1)))
        - m(0, 1)
            * (m(1, 0) * (m(2, 2) * m(3, 3) - m(2, 3) * m(3, 2))
                - m(1, 2) * (m(2, 0) * m(3, 3) - m(2, 3) * m(3, 0))
                + m(1, 3) * (m(2, 0) * m(3, 2) - m(2, 2) * m(3, 0)))
        + m(0, 2)
            * (m(1, 0) * (m(2, 1) * m(3, 3) - m(2, 3) * m(3, 1))
                - m(1, 1) * (m(2, 0) * m(3, 3) - m(2, 3) * m(3, 0))
                + m(1, 3) * (m(2, 0) * m(3, 1) - m(2, 1) * m(3, 0)))
        - m(0, 3)
            * (m(1, 0) * (m(2, 1) * m(3, 2) - m(2, 2) * m(3, 1))
                - m(1, 1) * (m(2, 0) * m(3, 2) - m(2, 2) * m(3, 0))
                + m(1, 2) * (m(2, 0) * m(3, 1) - m(2, 1) * m(3, 0)))
}

struct GLStateManager {
    current_viewport: (i32, i32, i32, i32),
    previous_viewport: (i32, i32, i32, i32),
    current_scissor: (i32, i32, i32, i32),
    previous_scissor: (i32, i32, i32, i32),
    viewport_changed: bool,
    scissor_changed: bool,
    scissor_was_explicitly_set: bool,
    last_gpu_verify: Option<((i32, i32, i32, i32), (i32, i32, i32, i32))>,
    scissor_test_enabled: bool,
    logical_size: (u32, u32),
    drawable_size: (u32, u32),
    change_count: u64,
    last_pipeline_log: Option<(
        (i32, i32, i32, i32),
        (i32, i32, i32, i32),
        (u32, u32),
        (u32, u32),
    )>,
}

static VIEWPORT_STATE_GLOBAL: OnceLock<Mutex<GLStateManager>> = OnceLock::new();
fn viewport_state() -> &'static Mutex<GLStateManager> {
    VIEWPORT_STATE_GLOBAL.get_or_init(|| {
        Mutex::new(GLStateManager {
            current_viewport: (0, 0, 0, 0),
            previous_viewport: (0, 0, 0, 0),
            current_scissor: (0, 0, 0, 0),
            previous_scissor: (0, 0, 0, 0),
            viewport_changed: false,
            scissor_changed: false,
            scissor_was_explicitly_set: false,
            last_gpu_verify: None,
            scissor_test_enabled: false,
            logical_size: (480, 320),
            drawable_size: (0, 0),
            change_count: 0,
            last_pipeline_log: None,
        })
    })
}

pub fn log_viewport_state_change(
    change_count: u64,
    old_viewport: (i32, i32, i32, i32),
    new_viewport: (i32, i32, i32, i32),
) {
    if !enabled() || old_viewport == new_viewport {
        return;
    }
    log!("[VIEWPORT CHANGE #{:04}] old=({},{},{},{}) new=({},{},{},{}) size_changed={} position_changed={}", change_count, old_viewport.0, old_viewport.1, old_viewport.2, old_viewport.3, new_viewport.0, new_viewport.1, new_viewport.2, new_viewport.3, old_viewport.2 != new_viewport.2 || old_viewport.3 != new_viewport.3, old_viewport.0 != new_viewport.0 || old_viewport.1 != new_viewport.1);
}

pub fn reset_state() {
    if !enabled() {
        return;
    }
    let mut state = viewport_state().lock().unwrap();
    state.current_viewport = (0, 0, 0, 0);
    state.previous_viewport = (0, 0, 0, 0);
    state.current_scissor = (0, 0, 0, 0);
    state.previous_scissor = (0, 0, 0, 0);
    state.viewport_changed = false;
    state.scissor_changed = false;
    state.scissor_was_explicitly_set = false;
    state.last_gpu_verify = None;
    state.change_count = 0;
    state.last_pipeline_log = None;
}

pub fn update_viewport_state(viewport: (i32, i32, i32, i32)) {
    if !enabled() {
        return;
    }
    let mut state = viewport_state().lock().unwrap();
    let old = state.current_viewport;
    state.previous_viewport = old;
    state.current_viewport = viewport;
    state.viewport_changed = old != viewport;
    if state.viewport_changed {
        state.change_count += 1;
    }
    let change_count = state.change_count;
    drop(state);
    if old != viewport {
        log_viewport_state_change(change_count, old, viewport);
    }
}

pub fn update_scissor_state(scissor: (i32, i32, i32, i32)) {
    if !enabled() {
        return;
    }
    let mut state = viewport_state().lock().unwrap();
    let old = state.current_scissor;
    state.previous_scissor = old;
    state.current_scissor = scissor;
    state.scissor_changed = old != scissor;
    state.scissor_was_explicitly_set = true;
    drop(state);
    if old != scissor && enabled() {
        log!(
            "[SCISSOR STATE CHANGE] old=({},{},{},{}) new=({},{},{},{})",
            old.0,
            old.1,
            old.2,
            old.3,
            scissor.0,
            scissor.1,
            scissor.2,
            scissor.3
        );
    }
}

pub fn set_scissor_test_enabled(value: bool) {
    if !enabled() {
        return;
    }
    let mut state = viewport_state().lock().unwrap();
    state.scissor_test_enabled = value;
}

pub fn update_scissor_test_enabled(value: bool) {
    set_scissor_test_enabled(value);
}

pub fn sync_scissor_to_viewport() -> Option<(i32, i32, i32, i32)> {
    if !enabled() {
        return None;
    }
    let mut state = viewport_state().lock().unwrap();
    if state.scissor_was_explicitly_set || state.current_scissor == state.current_viewport {
        return None;
    }
    let old = state.current_scissor;
    state.previous_scissor = old;
    state.current_scissor = state.current_viewport;
    state.scissor_changed = true;
    let viewport = state.current_viewport;
    drop(state);
    if enabled() {
        log!(
            "[SCISSOR SYNC] old=({},{},{},{}) new=({},{},{},{})",
            old.0,
            old.1,
            old.2,
            old.3,
            viewport.0,
            viewport.1,
            viewport.2,
            viewport.3
        );
    }
    Some(viewport)
}

pub fn log_pipeline_state(label: &str) {
    if !enabled() {
        return;
    }
    let mut state = viewport_state().lock().unwrap();
    let snapshot = (
        state.current_viewport,
        state.current_scissor,
        state.logical_size,
        state.drawable_size,
    );
    if !state.viewport_changed
        && !state.scissor_changed
        && state.last_pipeline_log == Some(snapshot)
    {
        return;
    }
    state.last_pipeline_log = Some(snapshot);
    let (x, y, width, height) = state.current_viewport;
    let (scissor_x, scissor_y, scissor_width, scissor_height) = state.current_scissor;
    log!("[COORDINATE PIPELINE] {} game_space={}x{} viewport=({},{},{},{}) scissor=({},{},{},{}) drawable={}x{} scale=({:.6},{:.6})", label, state.logical_size.0, state.logical_size.1, x, y, width, height, scissor_x, scissor_y, scissor_width, scissor_height, state.drawable_size.0, state.drawable_size.1, width as f32 / state.logical_size.0.max(1) as f32, height as f32 / state.logical_size.1.max(1) as f32);
    state.viewport_changed = false;
    state.scissor_changed = false;
}

pub fn log_coordinate_transformation_stack(label: &str) {
    log_pipeline_state(label);
}

pub fn log_projection_matrix(label: &str, matrix: &[f32; 16]) {
    if !enabled() {
        return;
    }
    let is_ortho = (matrix[15] - 1.0).abs() < f32::EPSILON;
    let is_perspective = (matrix[11].abs() - 1.0).abs() < f32::EPSILON;
    let kind = if is_ortho {
        "ORTHOGRAPHIC"
    } else if is_perspective {
        "PERSPECTIVE"
    } else {
        "UNKNOWN"
    };
    log!(
        "[PROJECTION MATRIX] label={} type={} determinant={:.6}",
        label,
        kind,
        determinant(matrix)
    );
    for row in 0..4 {
        log!(
            "[PROJECTION MATRIX] label={} row={} values=({:.6},{:.6},{:.6},{:.6})",
            label,
            row,
            matrix[row],
            matrix[row + 4],
            matrix[row + 8],
            matrix[row + 12]
        );
    }
    if is_ortho && matrix[0] != 0.0 && matrix[5] != 0.0 && matrix[10] != 0.0 {
        let left = (matrix[12] + 1.0) / matrix[0];
        let right = (matrix[12] - 1.0) / matrix[0];
        let bottom = (matrix[13] + 1.0) / matrix[5];
        let top = (matrix[13] - 1.0) / matrix[5];
        let near = (matrix[14] + 1.0) / matrix[10];
        let far = (matrix[14] - 1.0) / matrix[10];
        log!("[PROJECTION MATRIX] label={} bounds=left:{:.4},right:{:.4},bottom:{:.4},top:{:.4},near:{:.4},far:{:.4}", label, left, right, bottom, top, near, far);
    }
}

pub fn trace_viewport_usage(label: &str) {
    log_pipeline_state(label);
}

pub fn verify_viewport_state_in_gpu() {
    if !enabled() {
        return;
    }
    let mut viewport = [0i32; 4];
    let mut scissor = [0i32; 4];
    unsafe {
        gl::GetIntegerv(gl::VIEWPORT, viewport.as_mut_ptr());
        gl::GetIntegerv(gl::SCISSOR_BOX, scissor.as_mut_ptr());
    }
    let gpu_state = (viewport, scissor);
    let mut state = viewport_state().lock().unwrap();
    if state.last_gpu_verify
        == Some((
            (viewport[0], viewport[1], viewport[2], viewport[3]),
            (scissor[0], scissor[1], scissor[2], scissor[3]),
        ))
    {
        return;
    }
    state.last_gpu_verify = Some((
        (viewport[0], viewport[1], viewport[2], viewport[3]),
        (scissor[0], scissor[1], scissor[2], scissor[3]),
    ));
    drop(state);
    log!(
        "[GPU STATE VERIFY] viewport=({},{},{},{}) scissor=({},{},{},{})",
        gpu_state.0[0],
        gpu_state.0[1],
        gpu_state.0[2],
        gpu_state.0[3],
        gpu_state.1[0],
        gpu_state.1[1],
        gpu_state.1[2],
        gpu_state.1[3]
    );
}

pub fn log_gl_state_initialized() {
    if !enabled() || GL_STATE_SUMMARY_LOGGED.swap(true, Ordering::Relaxed) {
        return;
    }
    log!("[GL STATE MANAGER] Initialized");
    log!("[GL STATE MANAGER] glViewport and glScissor logs are change-only");
    log!("[GL STATE MANAGER] An unclaimed scissor is synchronized to the current viewport");
    log!("[GL STATE MANAGER] GPU verification runs only after a viewport change");
}

pub fn log_window_state(_orientation: &str, logical: (u32, u32), drawable: (u32, u32)) {
    if !enabled() {
        return;
    }
    let mut state = viewport_state().lock().unwrap();
    state.logical_size = logical;
    state.drawable_size = drawable;
    drop(state);
    log_pipeline_state("window generation");
}
