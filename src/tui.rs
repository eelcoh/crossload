//! Elm-style TUI using Tears, with blocking backend work isolated from update/view.
mod backend;
mod model;
#[cfg(test)]
mod tests;
mod view;
use anyhow::{ensure, Result};
use model::{Effect, Message, Model};
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
};
use tears::{Application, Command, Subscription};
#[derive(Clone)]
pub struct Options {
    pub show_previews: bool,
    pub device: Option<PathBuf>,
    pub browse: PathBuf,
    pub output: PathBuf,
    pub state: Option<PathBuf>,
    pub send_to: Option<String>,
    pub copy_to: Option<PathBuf>,
    pub folder: String,
    pub optimize: bool,
    pub organized: bool,
    pub serial: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    Directory(PathBuf),
    Local(PathBuf),
    Kobo(String),
}
#[derive(Clone, Debug)]
struct Entry {
    title: String,
    author: String,
    kind: &'static str,
    source: Source,
    preview: bool,
}

struct App {
    model: Model,
}
impl Application for App {
    type Message = Message;
    type Flags = Options;
    fn new(options: Options) -> (Self, Command<Message>) {
        let mut model = Model::new(options);
        let effects = model.update(Message::Refresh);
        (Self { model }, commands(effects))
    }
    fn update(&mut self, message: Message) -> Command<Message> {
        commands(self.model.update(message))
    }
    fn view(&self, frame: &mut ratatui::Frame<'_>) {
        view::draw(&self.model, frame);
    }
    fn subscriptions(&self) -> Vec<Subscription<Message>> {
        use tears::subscription::{terminal::TerminalEvents, time::Timer};
        let mut subs = vec![
            Subscription::new(TerminalEvents::new()).map(|event| match event {
                Ok(event) => Message::Input(event),
                Err(e) => Message::InputError(e.to_string()),
            }),
        ];
        if self.model.busy.is_some()
            || self.model.loading.is_some()
            || self.model.checking.is_some()
        {
            subs.push(Subscription::new(Timer::new(150)).map(|_| Message::Tick));
        }
        subs
    }
}
fn commands(effects: Vec<Effect>) -> Command<Message> {
    Command::batch(effects.into_iter().map(|effect| match effect {
        Effect::Quit => Command::effect(tears::Action::Quit),
        Effect::Check {
            id,
            options,
            entries,
        } => Command::future(async move {
            let result = blocking(move || backend::presence(&options, entries)).await;
            Message::Presence(
                id,
                result
                    .map_err(|e| format!("Inventory worker stopped: {e}"))
                    .and_then(|r| r.map_err(|e| format!("{e:#}"))),
            )
        }),
        Effect::Load { id, options, local } => Command::future(async move {
            let result = blocking(move || backend::load(&options, local)).await;
            Message::Loaded(
                id,
                result
                    .map_err(|e| format!("Directory worker stopped: {e}"))
                    .and_then(|r| r.map_err(|e| format!("{e:#}"))),
            )
        }),
        Effect::Work { id, options, entry } => Command::stream(work_stream(id, move |progress| {
            backend::perform(*options, entry, progress)
        })),
    }))
}
/// Start blocking work only when the command is polled. Its messages arrive in
/// order, and a worker panic becomes a completion error rather than a stuck job.
fn work_stream<F>(id: u64, work: F) -> impl futures::Stream<Item = Message> + Send
where
    F: FnOnce(&dyn Fn(&str)) -> Result<String> + Send + 'static,
{
    use futures::StreamExt;
    futures::stream::once(async move {
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            let progress_tx = tx.clone();
            let result = blocking(move || {
                work(&|value| {
                    let _ = progress_tx.blocking_send(Message::Progress(id, value.to_owned()));
                })
            })
            .await;
            let result = result
                .map_err(|e| {
                    format!("Book worker stopped: {e}; inspect local output before retrying")
                })
                .and_then(|r| r.map_err(|e| format!("{e:#}")));
            let _ = tx.send(Message::Finished(id, result)).await;
        });
        futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|message| (message, rx))
        })
    })
    .flatten()
}
thread_local! { static BACKGROUND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
async fn blocking<F, T>(work: F) -> Result<T, tokio::task::JoinError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                BACKGROUND.set(false);
            }
        }
        BACKGROUND.set(true);
        let _reset = Reset;
        work()
    })
    .await
}
type PanicHook = std::sync::Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync>;
struct Screen {
    previous_hook: PanicHook,
}
impl Screen {
    fn enter() -> Result<Self> {
        let previous_hook: PanicHook = std::panic::take_hook().into();
        let hook = previous_hook.clone();
        std::panic::set_hook(Box::new(move |info| {
            // Blocking worker panics become completion messages. Restoring the
            // terminal here would tear down a still-running UI.
            if BACKGROUND.get() {
                return;
            }
            ratatui::restore();
            hook(info);
        }));
        let screen = Self { previous_hook };
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(
            io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::cursor::Hide
        )?;
        Ok(screen)
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        ratatui::restore();
        // set_hook panics while unwinding. Normal/error returns restore it;
        // on a fatal UI panic the installed hook has already restored the screen.
        if !std::thread::panicking() {
            let hook = self.previous_hook.clone();
            std::panic::set_hook(Box::new(move |info| hook(info)));
        }
    }
}
pub fn run(options: Options) -> Result<()> {
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "The TUI needs an interactive terminal; use the CLI commands for redirected input/output"
    );
    if let Some(path) = &options.copy_to {
        ensure!(
            path.is_dir(),
            "--copy-to requires an existing mounted directory"
        );
    }
    if let Some(address) = &options.send_to {
        crate::crosspoint::Reader::new(address, &options.folder)?;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let screen = Screen::enter()?;
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))?;
    let result = runtime.block_on(async {
        tears::Runtime::<App>::new(options, 30)
            .run(&mut terminal)
            .await
    });
    drop(screen);
    // Runtime drop waits for started blocking operations even after terminal I/O failure.
    drop(runtime);
    result?;
    Ok(())
}
