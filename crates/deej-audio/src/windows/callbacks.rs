//! COM callback objects Windows calls when devices, sessions or volumes change.
//!
//! Windows invokes these on its own threads. Each one only posts a message to the engine thread and returns:
//! calling back into Core Audio from a callback (or blocking in one) can deadlock the audio service's
//! notification thread. The engine thread does the actual work.

use std::sync::mpsc::Sender;

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, AudioSessionDisconnectReason, AudioSessionState, AudioSessionStateExpired,
    DEVICE_STATE, EDataFlow, ERole, IAudioSessionControl, IAudioSessionEvents, IAudioSessionEvents_Impl,
    IAudioSessionNotification, IAudioSessionNotification_Impl, IMMNotificationClient, IMMNotificationClient_Impl,
    eCapture, eConsole, eRender,
};
use windows_core::{BOOL, GUID, PCWSTR, Ref, implement};

use super::util::pcwstr_to_string;
use super::{DEEJ_EVENT_CONTEXT, Msg};
use crate::model::{Flow, SessionId};

/// True if a change came from deej itself (it passes [`DEEJ_EVENT_CONTEXT`] to every volume call).
fn is_ours(context: *const GUID) -> bool {
    // SAFETY: Windows passes either null or a pointer to a GUID that's valid during the callback
    !context.is_null() && unsafe { *context } == DEEJ_EVENT_CONTEXT
}

#[implement(IMMNotificationClient)]
pub(crate) struct DeviceNotifier {
    pub(crate) tx: Sender<Msg>,
}

impl IMMNotificationClient_Impl for DeviceNotifier_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> windows_core::Result<()> {
        let _ = self.tx.send(Msg::DevicesChanged);
        Ok(())
    }

    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows_core::Result<()> {
        let _ = self.tx.send(Msg::DevicesChanged);
        Ok(())
    }

    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows_core::Result<()> {
        let _ = self.tx.send(Msg::DevicesChanged);
        Ok(())
    }

    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, id: &PCWSTR) -> windows_core::Result<()> {
        // deej's master and mic follow the console role, like the Go version. Windows sends one call per role
        if role != eConsole {
            return Ok(());
        }

        let flow = match flow {
            f if f == eRender => Flow::Render,
            f if f == eCapture => Flow::Capture,
            _ => return Ok(()),
        };

        let id = pcwstr_to_string(id);
        let _ = self.tx.send(Msg::DefaultChanged { flow, endpoint_id: (!id.is_empty()).then_some(id) });
        Ok(())
    }

    fn OnPropertyValueChanged(&self, _id: &PCWSTR, key: &PROPERTYKEY) -> windows_core::Result<()> {
        // this fires often for properties deej doesn't care about; only a renamed device matters
        if key.fmtid == PKEY_Device_FriendlyName.fmtid && key.pid == PKEY_Device_FriendlyName.pid {
            let _ = self.tx.send(Msg::DevicesChanged);
        }
        Ok(())
    }
}

#[implement(IAudioEndpointVolumeCallback)]
pub(crate) struct EndpointVolumeNotifier {
    pub(crate) tx: Sender<Msg>,
    pub(crate) endpoint_id: String,
}

impl IAudioEndpointVolumeCallback_Impl for EndpointVolumeNotifier_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows_core::Result<()> {
        if data.is_null() {
            return Ok(());
        }

        // SAFETY: Windows passes a valid notification struct for the duration of the call
        let data = unsafe { &*data };
        let _ = self.tx.send(Msg::EndpointVolume {
            endpoint_id: self.endpoint_id.clone(),
            volume: data.fMasterVolume,
            muted: data.bMuted.as_bool(),
            ours: is_ours(&data.guidEventContext),
        });
        Ok(())
    }
}

#[implement(IAudioSessionNotification)]
pub(crate) struct SessionNotifier {
    pub(crate) tx: Sender<Msg>,
    pub(crate) endpoint_id: String,
}

impl IAudioSessionNotification_Impl for SessionNotifier_Impl {
    fn OnSessionCreated(&self, _session: Ref<IAudioSessionControl>) -> windows_core::Result<()> {
        // the engine re-enumerates this device's sessions, which picks up the new one with everything it needs
        let _ = self.tx.send(Msg::SessionsChanged { endpoint_id: self.endpoint_id.clone() });
        Ok(())
    }
}

#[implement(IAudioSessionEvents)]
pub(crate) struct SessionEvents {
    pub(crate) tx: Sender<Msg>,
    pub(crate) session: SessionId,
}

impl IAudioSessionEvents_Impl for SessionEvents_Impl {
    fn OnDisplayNameChanged(&self, _name: &PCWSTR, _context: *const GUID) -> windows_core::Result<()> {
        let _ = self.tx.send(Msg::SessionChanged { session: self.session });
        Ok(())
    }

    fn OnIconPathChanged(&self, _path: &PCWSTR, _context: *const GUID) -> windows_core::Result<()> {
        Ok(())
    }

    fn OnSimpleVolumeChanged(&self, volume: f32, muted: BOOL, context: *const GUID) -> windows_core::Result<()> {
        let _ = self.tx.send(Msg::SessionVolume {
            session: self.session,
            volume,
            muted: muted.as_bool(),
            ours: is_ours(context),
        });
        Ok(())
    }

    fn OnChannelVolumeChanged(
        &self,
        _count: u32,
        _volumes: *const f32,
        _changed: u32,
        _context: *const GUID,
    ) -> windows_core::Result<()> {
        Ok(())
    }

    fn OnGroupingParamChanged(&self, _param: *const GUID, _context: *const GUID) -> windows_core::Result<()> {
        Ok(())
    }

    fn OnStateChanged(&self, state: AudioSessionState) -> windows_core::Result<()> {
        let msg = if state == AudioSessionStateExpired {
            Msg::SessionGone { session: self.session }
        } else {
            Msg::SessionChanged { session: self.session }
        };
        let _ = self.tx.send(msg);
        Ok(())
    }

    fn OnSessionDisconnected(&self, _reason: AudioSessionDisconnectReason) -> windows_core::Result<()> {
        let _ = self.tx.send(Msg::SessionGone { session: self.session });
        Ok(())
    }
}

pub(crate) fn device_notifier(tx: Sender<Msg>) -> IMMNotificationClient {
    DeviceNotifier { tx }.into()
}

pub(crate) fn endpoint_volume_notifier(tx: Sender<Msg>, endpoint_id: String) -> IAudioEndpointVolumeCallback {
    EndpointVolumeNotifier { tx, endpoint_id }.into()
}

pub(crate) fn session_notifier(tx: Sender<Msg>, endpoint_id: String) -> IAudioSessionNotification {
    SessionNotifier { tx, endpoint_id }.into()
}

pub(crate) fn session_events(tx: Sender<Msg>, session: SessionId) -> IAudioSessionEvents {
    SessionEvents { tx, session }.into()
}
