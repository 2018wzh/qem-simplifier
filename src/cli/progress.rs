use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use indicatif::{ProgressBar, ProgressStyle};

use crate::{
    qem_context_clear_progress_callback, qem_context_set_progress_callback, QemProgressEvent,
    QEM_PROGRESS_SCOPE_MESH, QEM_PROGRESS_SCOPE_SCENE, QEM_PROGRESS_STAGE_BEGIN,
    QEM_PROGRESS_STAGE_END, QEM_STATUS_SUCCESS,
};

#[derive(Clone, Copy, Debug)]
pub enum CliProgressScope {
    Mesh,
    Scene,
}

struct CliProgressState {
    bar: ProgressBar,
    scope: CliProgressScope,
    label: String,
    finished: AtomicBool,
}

pub struct CliProgressGuard {
    context: *mut c_void,
    state: *mut CliProgressState,
}

impl CliProgressGuard {
    pub fn attach(
        context: *mut c_void,
        scope: CliProgressScope,
        label: impl Into<String>,
    ) -> Result<Self, String> {
        if context.is_null() {
            return Err("qem context is null".to_string());
        }

        let bar = ProgressBar::new(1000);
        let style = ProgressStyle::with_template(
            "{spinner:.green} {msg} [{elapsed_precise}] [{bar:40.cyan/blue}] {percent:>3}%",
        )
        .map_err(|e| format!("failed to build progress style: {e}"))?
        .progress_chars("=>-");
        bar.set_style(style);

        let label = label.into();
        bar.set_message(format!("{}：准备中", label));

        let state = Box::new(CliProgressState {
            bar,
            scope,
            label,
            finished: AtomicBool::new(false),
        });
        let state_ptr = Box::into_raw(state);

        let status = unsafe {
            qem_context_set_progress_callback(context, cli_progress_callback, state_ptr as *mut c_void)
        };

        if status != QEM_STATUS_SUCCESS {
            unsafe {
                drop(Box::from_raw(state_ptr));
            }
            return Err(format!("qem_context_set_progress_callback failed: {status}"));
        }

        Ok(Self {
            context,
            state: state_ptr,
        })
    }

    pub fn finish_if_needed(&self, status: i32, success_message: &str, failure_message: &str) {
        if self.state.is_null() {
            return;
        }

        let state = unsafe { &*self.state };
        if state.finished.swap(true, Ordering::SeqCst) {
            return;
        }

        state.bar.set_position(1000);
        if status == QEM_STATUS_SUCCESS {
            state.bar.finish_with_message(success_message.to_string());
        } else {
            state
                .bar
                .abandon_with_message(format!("{} (status={})", failure_message, status));
        }
    }
}

impl Drop for CliProgressGuard {
    fn drop(&mut self) {
        if self.context.is_null() || self.state.is_null() {
            return;
        }

        let _ = unsafe { qem_context_clear_progress_callback(self.context) };

        let state = unsafe { Box::from_raw(self.state) };
        if !state.finished.load(Ordering::SeqCst) {
            state.bar.finish_and_clear();
        }

        self.state = std::ptr::null_mut();
    }
}

unsafe extern "C" fn cli_progress_callback(event: *const QemProgressEvent, user_data: *mut c_void) {
    if event.is_null() || user_data.is_null() {
        return;
    }

    let ev = unsafe { &*event };
    let state = unsafe { &*(user_data as *const CliProgressState) };

    let scope_matches = match state.scope {
        CliProgressScope::Mesh => ev.scope == QEM_PROGRESS_SCOPE_MESH,
        CliProgressScope::Scene => ev.scope == QEM_PROGRESS_SCOPE_SCENE,
    };

    if !scope_matches {
        return;
    }

    let pct = ev.percent.clamp(0.0, 1.0);
    state.bar.set_position((pct * 1000.0).round() as u64);

    match state.scope {
        CliProgressScope::Mesh => {
            state.bar.set_message(format!(
                "{}：src={} target={} out={}",
                state.label, ev.source_triangles, ev.target_triangles, ev.output_triangles
            ));
        }
        CliProgressScope::Scene => {
            state.bar.set_message(format!(
                "{}：mesh {}/{} | src={} target={} out={}",
                state.label,
                ev.mesh_index.saturating_add(1),
                ev.mesh_count,
                ev.source_triangles,
                ev.target_triangles,
                ev.output_triangles
            ));
        }
    }

    if ev.stage == QEM_PROGRESS_STAGE_BEGIN {
        state.bar.set_position(0);
    }

    if ev.stage == QEM_PROGRESS_STAGE_END {
        if !state.finished.swap(true, Ordering::SeqCst) {
            if ev.status == QEM_STATUS_SUCCESS {
                state
                    .bar
                    .finish_with_message(format!("{}：完成", state.label));
            } else {
                state
                    .bar
                    .abandon_with_message(format!("{}：失败(status={})", state.label, ev.status));
            }
        }
    }
}
