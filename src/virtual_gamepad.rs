use anyhow::{Context, Result};
use vigem_rust::{target::Xbox360, Client, TargetHandle, X360Report};

pub struct VirtualGamepad {
    // Client must outlive target. Target stores a weak reference to it.
    _client: Client,
    target: TargetHandle<Xbox360>,
}

impl VirtualGamepad {
    pub fn connect() -> Result<Self> {
        let client = Client::connect().context("не удалось подключиться к ViGEmBus")?;
        let target = client
            .new_x360_target()
            .plugin()
            .context("не удалось подключить виртуальный контроллер Xbox 360")?;
        target
            .wait_for_ready()
            .context("виртуальный контроллер Xbox 360 не стал готов")?;
        Ok(Self {
            _client: client,
            target,
        })
    }

    pub fn update(&self, report: &X360Report) -> Result<()> {
        self.target
            .update(report)
            .context("не удалось обновить виртуальный контроллер Xbox 360")
    }
}
