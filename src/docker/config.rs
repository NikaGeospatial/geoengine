use std::collections::HashMap;

use crate::docker::gpu::GpuConfig;

pub struct ContainerConfig {
    pub image: String,
    pub command: Option<Vec<String>>,
    pub env_vars: HashMap<String, String>,
    pub mounts: Vec<(String, String, bool)>, // (host, container, readonly)
    pub gpu_config: Option<GpuConfig>,
    pub workdir: Option<String>,
    pub name: Option<String>,
    pub remove_on_exit: bool,
    pub detach: bool,
    pub tty: bool,
}
