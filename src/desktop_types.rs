use crate::app::{Effort, ProviderKind};

#[derive(Debug, Clone)]
pub enum DesktopAction {
    Browse(crate::project_files::Request),
    RefreshFiles,
    CloseFilePreview,
    AttachFile {
        panel_id: u64,
        path: String,
    },
    RemoveAttachment {
        panel_id: u64,
        path: String,
    },
    ReviewEdit {
        panel_id: u64,
        approve: bool,
    },
    Send(u64),
    Cancel(u64),
    NewPanel,
    ClosePanel(u64),
    Login,
    CancelLogin,
    Logout,
    ConnectCodexAccount,
    CancelCodexAccount,
    DisconnectCodexAccount,
    RefreshModels,
    RefreshUsage,
    RefreshAntigravityUsage,
    SetOpenRouterKey(String),
    OpenCodex {
        model: String,
    },
    OpenAntigravity,
    RestartCodex(u64),
    OpenAgent {
        title: String,
        model: String,
    },
    SetProvider {
        panel_id: u64,
        provider: ProviderKind,
    },
    SetModel {
        panel_id: u64,
        model: String,
    },
    SetEffort {
        panel_id: u64,
        effort: Effort,
    },
    OpenProject,
    SwitchProject(std::path::PathBuf),
    SelectProjectPanel {
        project: std::path::PathBuf,
        panel_id: u64,
    },
    SetProjectPinned {
        project: std::path::PathBuf,
        pinned: bool,
    },
    RemoveProject(std::path::PathBuf),
    Save,
    CheckUpdates,
}
