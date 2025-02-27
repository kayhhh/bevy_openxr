mod graphics;
pub mod input;
pub mod resource_macros;
pub mod resources;
pub mod xr_init;
pub mod xr_input;

use std::sync::{Arc, Mutex};

use crate::xr_init::RenderRestartPlugin;
use crate::xr_input::hands::hand_tracking::DisableHandTracking;
use crate::xr_input::oculus_touch::ActionSets;
use bevy::app::PluginGroupBuilder;
use bevy::ecs::system::{RunSystemOnce, SystemState};
use bevy::prelude::*;
use bevy::render::camera::{
    CameraPlugin, ManualTextureView, ManualTextureViewHandle, ManualTextureViews,
};
use bevy::render::globals::GlobalsPlugin;
use bevy::render::mesh::morph::MorphPlugin;
use bevy::render::mesh::MeshPlugin;
use bevy::render::pipelined_rendering::PipelinedRenderingPlugin;
use bevy::render::render_asset::RenderAssetDependency;
use bevy::render::render_resource::ShaderLoader;
use bevy::render::renderer::{
    render_system, RenderAdapter, RenderAdapterInfo, RenderDevice, RenderInstance, RenderQueue,
};
use bevy::render::settings::RenderCreation;
use bevy::render::view::{self, ViewPlugin, WindowRenderPlugin};
use bevy::render::{color, primitives, Render, RenderApp, RenderPlugin, RenderSet};
use bevy::window::{PresentMode, PrimaryWindow, RawHandleWrapper};
use input::XrInput;
use openxr as xr;
use resources::*;
use xr::FormFactor;
use xr_init::{
    init_non_xr_graphics, update_xr_stuff, xr_only, RenderCreationData, XrEnableRequest,
    XrEnableStatus, XrRenderData, XrRenderUpdate,
};
use xr_input::controllers::XrControllerType;
use xr_input::hands::emulated::HandEmulationPlugin;
use xr_input::hands::hand_tracking::{HandTrackingData, HandTrackingPlugin};
use xr_input::OpenXrInput;

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

pub const LEFT_XR_TEXTURE_HANDLE: ManualTextureViewHandle = ManualTextureViewHandle(1208214591);
pub const RIGHT_XR_TEXTURE_HANDLE: ManualTextureViewHandle = ManualTextureViewHandle(3383858418);

/// Adds OpenXR support to an App
pub struct OpenXrPlugin;

impl Default for OpenXrPlugin {
    fn default() -> Self {
        OpenXrPlugin
    }
}

#[derive(Resource)]
pub struct FutureXrResources(
    pub  Arc<
        Mutex<
            Option<(
                XrInstance,
                XrSession,
                XrEnvironmentBlendMode,
                XrResolution,
                XrFormat,
                XrSessionRunning,
                XrFrameWaiter,
                XrSwapchain,
                XrInput,
                XrViews,
                XrFrameState,
            )>,
        >,
    >,
);

impl Plugin for OpenXrPlugin {
    fn build(&self, app: &mut App) {
        let mut system_state: SystemState<Query<&RawHandleWrapper, With<PrimaryWindow>>> =
            SystemState::new(&mut app.world);
        let primary_window = system_state.get(&app.world).get_single().ok().cloned();

        #[cfg(not(target_arch = "wasm32"))]
        match graphics::initialize_xr_graphics(primary_window.clone()) {
            Ok((
                device,
                queue,
                adapter_info,
                render_adapter,
                instance,
                xr_instance,
                session,
                blend_mode,
                resolution,
                format,
                session_running,
                frame_waiter,
                swapchain,
                input,
                views,
                frame_state,
            )) => {
                // std::thread::sleep(Duration::from_secs(5));
                debug!("Configured wgpu adapter Limits: {:#?}", device.limits());
                debug!("Configured wgpu adapter Features: {:#?}", device.features());
                app.insert_resource(xr_instance.clone());
                app.insert_resource(session.clone());
                app.insert_resource(blend_mode.clone());
                app.insert_resource(resolution.clone());
                app.insert_resource(format.clone());
                app.insert_resource(session_running.clone());
                app.insert_resource(frame_waiter.clone());
                app.insert_resource(swapchain.clone());
                app.insert_resource(input.clone());
                app.insert_resource(views.clone());
                app.insert_resource(frame_state.clone());
                let xr_data = XrRenderData {
                    xr_instance,
                    xr_session: session,
                    xr_blend_mode: blend_mode,
                    xr_resolution: resolution,
                    xr_format: format,
                    xr_session_running: session_running,
                    xr_frame_waiter: frame_waiter,
                    xr_swapchain: swapchain,
                    xr_input: input,
                    xr_views: views,
                    xr_frame_state: frame_state,
                };
                app.insert_resource(xr_data);
                app.insert_resource(ActionSets(vec![]));
                app.add_plugins(RenderPlugin {
                    render_creation: RenderCreation::Manual(
                        device,
                        queue,
                        adapter_info,
                        render_adapter,
                        RenderInstance(Arc::new(instance)),
                    ),
                });
                app.insert_resource(XrEnableStatus::Enabled);
            }
            Err(err) => {
                warn!("OpenXR Failed to initialize: {}", err);
                app.add_plugins(RenderPlugin::default());
                app.insert_resource(XrEnableStatus::Disabled);
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            app.add_plugins(RenderPlugin::default());
            app.insert_resource(XrEnableStatus::Disabled);
        }
    }

    fn ready(&self, app: &App) -> bool {
        app.world
            .get_resource::<XrEnableStatus>()
            .map(|frr| *frr != XrEnableStatus::Waiting)
            .unwrap_or(true)
    }

    fn finish(&self, app: &mut App) {
        // TODO: Split this up into the indevidual resources
        if let Some(data) = app.world.get_resource::<XrRenderData>().cloned() {
            let hands = data.xr_instance.exts().ext_hand_tracking.is_some()
                && data
                    .xr_instance
                    .supports_hand_tracking(
                        data.xr_instance
                            .system(FormFactor::HEAD_MOUNTED_DISPLAY)
                            .unwrap(),
                    )
                    .is_ok_and(|v| v);
            if hands {
                app.insert_resource(HandTrackingData::new(&data.xr_session).unwrap());
            } else {
                app.insert_resource(DisableHandTracking::Both);
            }

            let (left, right) = data.xr_swapchain.get_render_views();
            let left = ManualTextureView {
                texture_view: left.into(),
                size: *data.xr_resolution,
                format: *data.xr_format,
            };
            let right = ManualTextureView {
                texture_view: right.into(),
                size: *data.xr_resolution,
                format: *data.xr_format,
            };
            app.add_systems(PreUpdate, xr_begin_frame.run_if(xr_only()));
            let mut manual_texture_views = app.world.resource_mut::<ManualTextureViews>();
            manual_texture_views.insert(LEFT_XR_TEXTURE_HANDLE, left);
            manual_texture_views.insert(RIGHT_XR_TEXTURE_HANDLE, right);
            drop(manual_texture_views);
            let render_app = app.sub_app_mut(RenderApp);

            render_app.insert_resource(data.xr_instance.clone());
            render_app.insert_resource(data.xr_session.clone());
            render_app.insert_resource(data.xr_blend_mode.clone());
            render_app.insert_resource(data.xr_resolution.clone());
            render_app.insert_resource(data.xr_format.clone());
            render_app.insert_resource(data.xr_session_running.clone());
            render_app.insert_resource(data.xr_frame_waiter.clone());
            render_app.insert_resource(data.xr_swapchain.clone());
            render_app.insert_resource(data.xr_input.clone());
            render_app.insert_resource(data.xr_views.clone());
            render_app.insert_resource(data.xr_frame_state.clone());
            render_app.insert_resource(XrEnableStatus::Enabled);
            render_app.add_systems(
                Render,
                (
                    post_frame
                        .run_if(xr_only())
                        .before(render_system)
                        .after(RenderSet::ExtractCommands),
                    end_frame.run_if(xr_only()).after(render_system),
                ),
            );
        }
    }
}

pub struct DefaultXrPlugins;

impl PluginGroup for DefaultXrPlugins {
    fn build(self) -> PluginGroupBuilder {
        DefaultPlugins
            .build()
            .disable::<RenderPlugin>()
            .disable::<PipelinedRenderingPlugin>()
            .add_before::<RenderPlugin, _>(OpenXrPlugin)
            .add_after::<OpenXrPlugin, _>(OpenXrInput::new(XrControllerType::OculusTouch))
            .add_before::<OpenXrPlugin, _>(RenderRestartPlugin)
            .add(HandEmulationPlugin)
            .add(HandTrackingPlugin)
            .set(WindowPlugin {
                #[cfg(not(target_os = "android"))]
                primary_window: Some(Window {
                    present_mode: PresentMode::AutoNoVsync,
                    ..default()
                }),
                #[cfg(target_os = "android")]
                primary_window: None,
                #[cfg(target_os = "android")]
                exit_condition: bevy::window::ExitCondition::DontExit,
                #[cfg(target_os = "android")]
                close_when_requested: true,
                ..default()
            })
    }
}

pub fn xr_begin_frame(
    instance: Res<XrInstance>,
    session: Res<XrSession>,
    session_running: Res<XrSessionRunning>,
    frame_state: Res<XrFrameState>,
    frame_waiter: Res<XrFrameWaiter>,
    swapchain: Res<XrSwapchain>,
    views: Res<XrViews>,
    input: Res<XrInput>,
) {
    {
        let _span = info_span!("xr_poll_events");
        while let Some(event) = instance.poll_event(&mut Default::default()).unwrap() {
            use xr::Event::*;
            match event {
                SessionStateChanged(e) => {
                    // Session state change is where we can begin and end sessions, as well as
                    // find quit messages!
                    info!("entered XR state {:?}", e.state());
                    match e.state() {
                        xr::SessionState::READY => {
                            session.begin(VIEW_TYPE).unwrap();
                            session_running.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        xr::SessionState::STOPPING => {
                            session.end().unwrap();
                            session_running.store(false, std::sync::atomic::Ordering::Relaxed);
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => return,
                        _ => {}
                    }
                }
                InstanceLossPending(_) => return,
                EventsLost(e) => {
                    warn!("lost {} XR events", e.lost_event_count());
                }
                _ => {}
            }
        }
    }
    {
        let _span = info_span!("xr_wait_frame").entered();
        *frame_state.lock().unwrap() = match frame_waiter.lock().unwrap().wait() {
            Ok(a) => a,
            Err(e) => {
                warn!("error: {}", e);
                return;
            }
        };
    }
    {
        let _span = info_span!("xr_begin_frame").entered();
        swapchain.begin().unwrap()
    }
    {
        let _span = info_span!("xr_locate_views").entered();
        *views.lock().unwrap() = session
            .locate_views(
                VIEW_TYPE,
                frame_state.lock().unwrap().predicted_display_time,
                &input.stage,
            )
            .unwrap()
            .1;
    }
}

pub fn post_frame(
    resolution: Res<XrResolution>,
    format: Res<XrFormat>,
    swapchain: Res<XrSwapchain>,
    mut manual_texture_views: ResMut<ManualTextureViews>,
) {
    {
        let _span = info_span!("xr_acquire_image").entered();
        swapchain.acquire_image().unwrap()
    }
    {
        let _span = info_span!("xr_wait_image").entered();
        swapchain.wait_image().unwrap();
    }
    {
        let _span = info_span!("xr_update_manual_texture_views").entered();
        let (left, right) = swapchain.get_render_views();
        let left = ManualTextureView {
            texture_view: left.into(),
            size: **resolution,
            format: **format,
        };
        let right = ManualTextureView {
            texture_view: right.into(),
            size: **resolution,
            format: **format,
        };
        manual_texture_views.insert(LEFT_XR_TEXTURE_HANDLE, left);
        manual_texture_views.insert(RIGHT_XR_TEXTURE_HANDLE, right);
    }
}

pub fn end_frame(
    xr_frame_state: Res<XrFrameState>,
    views: Res<XrViews>,
    input: Res<XrInput>,
    swapchain: Res<XrSwapchain>,
    resolution: Res<XrResolution>,
    environment_blend_mode: Res<XrEnvironmentBlendMode>,
) {
    {
        let _span = info_span!("xr_release_image").entered();
        swapchain.release_image().unwrap();
    }
    {
        let _span = info_span!("xr_end_frame").entered();
        let result = swapchain.end(
            xr_frame_state.lock().unwrap().predicted_display_time,
            &*views.lock().unwrap(),
            &input.stage,
            **resolution,
            **environment_blend_mode,
        );
        match result {
            Ok(_) => {}
            Err(e) => warn!("error: {}", e),
        }
    }
}

pub fn locate_views(
    views: Res<XrViews>,
    input: Res<XrInput>,
    session: Res<XrSession>,
    xr_frame_state: Res<XrFrameState>,
) {
    let _span = info_span!("xr_locate_views").entered();
    *views.lock().unwrap() = match session.locate_views(
        VIEW_TYPE,
        xr_frame_state.lock().unwrap().predicted_display_time,
        &input.stage,
    ) {
        Ok(this) => this,
        Err(err) => {
            warn!("error: {}", err);
            return;
        }
    }
    .1;
}
