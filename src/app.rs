use std::{sync::Arc, path::PathBuf};

use druid::{
  commands,
  keyboard_types::Key,
  lens,
  widget::{
    Axis, Button, Checkbox, Controller, Flex, Label, Scope, ScopeTransfer, Tabs, TabsPolicy,
    TextBox, ViewSwitcher, Painter, Maybe, SizedBox,
  },
  AppDelegate as Delegate, Command, Data, DelegateCtx, Env, Event, EventCtx, Handled, KeyEvent,
  Lens, LensExt, Menu, MenuItem, Selector, Target, Widget, WidgetExt, WidgetId, WindowDesc,
  WindowId, RenderContext, theme, WindowConfig,
};
use druid_widget_nursery::{WidgetExt as WidgetExtNursery, material_icons::Icon};
use rfd::{AsyncFileDialog, FileHandle};
use strum::IntoEnumIterator;
use tap::Tap;
use tokio::runtime::Handle;
use lazy_static::lazy_static;

use crate::patch::{
  split::Split,
  tabs_policy::{InitialTab, StaticTabsForked},
};

use self::{
  mod_description::ModDescription,
  mod_entry::ModEntry,
  mod_list::{EnabledMods, Filters, ModList},
  settings::{Settings, SettingsCommand},
  util::{h2, h3, LabelExt, icons::*, GET_INSTALLED_STARSECTOR, get_starsector_version, get_quoted_version, make_column_pair, DragWindowController}, installer::{ChannelMessage, StringOrPath},
};

mod installer;
mod mod_description;
mod mod_entry;
mod mod_list;
mod settings;
#[path = "./util.rs"]
pub mod util;

#[derive(Clone, Data, Lens)]
pub struct App {
  init: bool,
  settings: settings::Settings,
  mod_list: mod_list::ModList,
  active: Option<Arc<ModEntry>>,
  #[data(ignore)]
  runtime: Handle,
  #[data(ignore)]
  widget_id: WidgetId,
}

impl App {
  const SELECTOR: Selector<AppCommands> = Selector::new("app.update.commands");
  const OPEN_FILE: Selector<Option<Vec<FileHandle>>> = Selector::new("app.open.multiple");
  const OPEN_FOLDER: Selector<Option<FileHandle>> = Selector::new("app.open.folder");
  const ENABLE: Selector<()> = Selector::new("app.enable");
  const DUMB_UNIVERSAL_ESCAPE: Selector<()> = Selector::new("app.universal_escape");
  const REFRESH: Selector<()> = Selector::new("app.mod_list.refresh");
  const DISABLE: Selector<()> = Selector::new("app.disable");

  pub fn new(handle: Handle) -> Self {
    App {
      init: false,
      settings: settings::Settings::load()
        .map(|mut settings| {
          if settings.vmparams_enabled {
            if let Some(path) = settings.install_dir.clone() {
              settings.vmparams = settings::vmparams::VMParams::load(path).ok();
            }
          }
          if let Some(install_dir) = settings.install_dir.clone() {
            settings.install_dir_buf = install_dir.to_string_lossy().to_string()
          }
          settings
        })
        .unwrap_or_else(|_| settings::Settings::default()),
      mod_list: mod_list::ModList::new(),
      active: None,
      runtime: handle,
      widget_id: WidgetId::reserved(0),
    }
  }

  pub fn ui_builder() -> impl Widget<Self> {
    let button_painter = || Painter::new(|ctx, _, env| {
      let is_active = ctx.is_active() && !ctx.is_disabled();
      let is_hot = ctx.is_hot();
      let size = ctx.size();
      let stroke_width = env.get(theme::BUTTON_BORDER_WIDTH);
      
      let rounded_rect = size
      .to_rect()
      .inset(-stroke_width / 2.0)
      .to_rounded_rect(env.get(theme::BUTTON_BORDER_RADIUS));
      
      let bg_gradient = if ctx.is_disabled() {
        env.get(theme::DISABLED_BUTTON_DARK)
      } else if is_active {
        env.get(theme::BUTTON_DARK)
      } else {
        env.get(theme::BUTTON_LIGHT)
      };
      
      let border_color = if is_hot && !ctx.is_disabled() {
        env.get(theme::BORDER_LIGHT)
      } else {
        env.get(theme::BORDER_DARK)
      };
      
      ctx.stroke(rounded_rect, &border_color, stroke_width);

      ctx.fill(rounded_rect, &bg_gradient);
    });
    let settings = Flex::row()
      .with_child(Flex::row()
        .with_child(Label::new("Settings").with_text_size(18.))
        .with_spacer(5.)
        .with_child(Icon::new(SETTINGS))
        .padding((8., 4.))
        .background(button_painter())
        .on_click(|event_ctx, _, _| {
          event_ctx.submit_command(App::SELECTOR.with(AppCommands::OpenSettings))
        }
      ))
      .expand_width();
    let refresh = Flex::row()
      .with_child(Flex::row()
        .with_child(Label::new("Refresh").with_text_size(18.))
        .with_spacer(5.)
        .with_child(Icon::new(SYNC))
        .padding((8., 4.))
        .background(button_painter())
        .on_click(|event_ctx, _, _| {
          event_ctx.submit_command(App::REFRESH)
        }
      ))
      .expand_width();
    let install_dir_browser = Settings::install_dir_browser_builder(Axis::Vertical)
      .lens(App::settings);
    let install_mod_button = Flex::row()
      .with_child(Label::new("Install Mod(s)").with_text_size(18.))
      .with_spacer(5.)
      .with_child(Icon::new(INSTALL_DESKTOP))
      .padding((8., 4.))
      .background(button_painter())
      .on_click(|_, _, _| {})
      .controller(InstallController)
      .on_command(App::OPEN_FILE, |ctx, payload, data| {
        if let Some(targets) = payload {
          data.runtime.spawn(
            installer::Payload::Initial(
              targets
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect(),
            )
            .install(
              ctx.get_external_handle(),
              data.settings.install_dir.clone().unwrap(),
              data.mod_list.mods.values().map(|v| v.id.clone()).collect(),
            ),
          );
        }
      })
      .on_command(App::OPEN_FOLDER, |ctx, payload, data| {
        if let Some(target) = payload {
          data.runtime.spawn(
            installer::Payload::Initial(vec![target.path().to_path_buf()]).install(
              ctx.get_external_handle(),
              data.settings.install_dir.clone().unwrap(),
              data.mod_list.mods.values().map(|v| v.id.clone()).collect(),
            ),
          );
        }
      });
    let mod_list = mod_list::ModList::ui_builder()
      .lens(App::mod_list)
      .on_change(|_ctx, _old, data, _env| {
        if let Some(install_dir) = &data.settings.install_dir {
          let enabled: Vec<Arc<ModEntry>> = data
            .mod_list
            .mods
            .iter()
            .filter_map(|(_, v)| v.enabled.then(|| v.clone()))
            .collect();

          if let Err(err) = EnabledMods::from(enabled).save(install_dir) {
            eprintln!("{:?}", err)
          };
        }
      })
      .expand()
      .controller(ModListController);
    let mod_description = ViewSwitcher::new(
      |active: &Option<Arc<ModEntry>>, _| active.clone(),
      |active, _, _| {
        if let Some(active) = active {
          Box::new(ModDescription::ui_builder().lens(lens::Constant(active.clone())))
        } else {
          Box::new(ModDescription::empty_builder().lens(lens::Unit))
        }
      },
    )
    .lens(App::active);
    let tool_panel = Flex::column()
      .cross_axis_alignment(druid::widget::CrossAxisAlignment::Start)
      .with_child(h2("Search"))
      .with_child(
        TextBox::new()
          .on_change(|ctx, _, _, _| {
            ctx.submit_command(ModList::SEARCH_UPDATE);
          })
          .lens(App::mod_list.then(ModList::search_text))
          .expand_width(),
      )
      .with_default_spacer()
      .with_child(h2("Toggles"))
      .with_child(
        Button::new("Enable All")
          .disabled_if(|data: &App, _| data.mod_list.mods.values().all(|e| e.enabled))
          .on_click(|_, data: &mut App, _| {
            if let Some(install_dir) = data.settings.install_dir.as_ref() {
              let mut enabled: Vec<String> = Vec::new();
              data.mod_list.mods = data
                .mod_list
                .mods
                .drain_filter(|_, _| true)
                .map(|(id, mut entry)| {
                  (Arc::make_mut(&mut entry)).enabled = true;
                  enabled.push(id.clone());
                  (id, entry)
                })
                .collect();
              if let Err(err) = EnabledMods::from(enabled).save(install_dir) {
                eprintln!("{:?}", err)
              }
            }
          })
          .expand_width(),
      )
      .with_spacer(5.)
      .with_child(
        Button::new("Disable All")
          .disabled_if(|data: &App, _| data.mod_list.mods.values().all(|e| !e.enabled))
          .on_click(|_, data: &mut App, _| {
            if let Some(install_dir) = data.settings.install_dir.as_ref() {
              data.mod_list.mods = data
                .mod_list
                .mods
                .drain_filter(|_, _| true)
                .map(|(id, mut entry)| {
                  (Arc::make_mut(&mut entry)).enabled = false;
                  (id, entry)
                })
                .collect();
              if let Err(err) = EnabledMods::empty().save(install_dir) {
                eprintln!("{:?}", err)
              }
            }
          })
          .expand_width(),
      )
      .with_default_spacer()
      .with_child(h2("Filters"))
      .tap_mut(|panel| {
        for filter in Filters::iter() {
          match filter {
            Filters::Enabled => panel.add_child(h3("Status")),
            Filters::Unimplemented => panel.add_child(h3("Version Checker")),
            Filters::AutoUpdateAvailable => panel.add_child(h3("Auto Update Support")),
            _ => {}
          };
          panel.add_child(
            Scope::from_function(
              |state: bool| state,
              IndyToggleState { state: true },
              Checkbox::from_label(Label::wrapped(&filter.to_string())).on_change(
                move |ctx, _, new, _| {
                  ctx.submit_command(ModList::FILTER_UPDATE.with((filter, !*new)))
                },
              ),
            )
            .lens(lens::Constant(true)),
          )
        }
      })
      .padding(20.);
    let launch_panel = Flex::column()
      .with_child(make_column_pair(
        h2("Starsector Version:"),
        Maybe::new(
          || Label::wrapped_func(|v: &String, _| v.clone()),
          || Label::new("Unknown")
        ).lens(App::mod_list.then(ModList::starsector_version).map(
          |v| v.as_ref().and_then(|v| get_quoted_version(v)),
          |_, _| {}
        ))
      ))
      .with_default_spacer()
      .with_child(install_dir_browser)
      .with_default_spacer()
      .with_child(ViewSwitcher::new(
        |data: &App, _| data.settings.install_dir.is_some(),
        move |has_dir, _, _| {
          if *has_dir {
            Box::new(
              Flex::row()
                .with_flex_child(h2("Launch Starsector").expand_width(), 2.)
                .with_flex_child(Icon::new(PLAY_ARROW).expand_width(), 1.)
                .padding((8., 4.))
                .background(button_painter())
                .on_click(|ctx, data: &mut App, _| {
                  if let Some(install_dir) = data.settings.install_dir.clone() {
                    ctx.submit_command(App::DISABLE);
                    let ext_ctx = ctx.get_external_handle();
                    let experimental_launch = data.settings.experimental_launch;
                    let resolution = data.settings.experimental_resolution;
                    data.runtime.spawn(async move {
                      if let Err(err) = App::launch_starsector(install_dir, experimental_launch, resolution).await {
                        dbg!(err);
                      };
                      ext_ctx.submit_command(App::ENABLE, (), Target::Auto)
                    });
                  }
                })
                .expand_width()
            )
          } else {
            Box::new(SizedBox::empty())
          }
        }
      ))
      .main_axis_alignment(druid::widget::MainAxisAlignment::Start)
      .expand()
      .padding(20.);
    let side_panel = Tabs::for_policy(
      StaticTabsForked::build(vec![
        InitialTab::new("Tools & Filters", tool_panel),
        InitialTab::new("Launch", launch_panel),
      ])
      .set_label_height(40.0),
    );

    Flex::column()
      .with_child(
        Flex::row()
          .with_child(settings)
          .with_spacer(10.)
          .with_child(install_mod_button)
          .with_spacer(10.)
          .with_child(refresh)
          .with_spacer(10.)
          .with_child(ViewSwitcher::new(
            |len: &usize, _| *len,
            |len, _, _| Box::new(h3(&format!("Installed: {}", len)))
          ).lens(App::mod_list.then(ModList::mods).map(
            |data| data.len(),
            |_, _| {}
          )))
          .with_spacer(10.)
          .with_child(ViewSwitcher::new(
            |len: &usize, _| *len,
            |len, _, _| Box::new(h3(&format!("Active: {}", len)))
          ).lens(App::mod_list.then(ModList::mods).map(
            |data| data.values().filter(|e| e.enabled).count(),
            |_, _| {}
          )))
          .main_axis_alignment(druid::widget::MainAxisAlignment::Start)
          .expand_width(),
      )
      .with_spacer(20.)
      .with_flex_child(
        Split::columns(mod_list, side_panel)
          .split_point(0.8)
          .draggable(true)
          .expand_height(),
        2.0,
      )
      .cross_axis_alignment(druid::widget::CrossAxisAlignment::Start)
      .with_flex_child(mod_description, 1.0)
      .must_fill_main_axis(true)
      .controller(AppController)
      .with_id(WidgetId::reserved(0))
  }

  async fn launch_starsector(install_dir: PathBuf, experimental_launch: bool, resolution: (u32, u32)) -> Result<(), String> {
    use tokio::process::Command;
    use tokio::fs::read_to_string;

    lazy_static! {
      static ref JAVA_REGEX: regex::Regex = regex::Regex::new(r"java\.exe").expect("compile regex");
    }

    let child = if experimental_launch {
      // let mut args_raw = String::from(r"java.exe -XX:CompilerThreadPriority=1 -XX:+CompilerThreadHintNoPreempt -XX:+DisableExplicitGC -XX:+UnlockExperimentalVMOptions -XX:+AggressiveOpts -XX:+TieredCompilation -XX:+UseG1GC -XX:InitialHeapSize=2048m -XX:MaxMetaspaceSize=2048m -XX:MaxNewSize=2048m -XX:+ParallelRefProcEnabled -XX:G1NewSizePercent=5 -XX:G1MaxNewSizePercent=10 -XX:G1ReservePercent=5 -XX:G1MixedGCLiveThresholdPercent=70 -XX:InitiatingHeapOccupancyPercent=90 -XX:G1HeapWastePercent=5 -XX:MaxGCPauseMillis=50 -XX:G1HeapRegionSize=2M -XX:+UseStringDeduplication -Djava.library.path=native\windows -Xms1536m -Xmx1536m -Xss2048k -classpath janino.jar;commons-compiler.jar;commons-compiler-jdk.jar;starfarer.api.jar;starfarer_obf.jar;jogg-0.0.7.jar;jorbis-0.0.15.jar;json.jar;lwjgl.jar;jinput.jar;log4j-1.2.9.jar;lwjgl_util.jar;fs.sound_obf.jar;fs.common_obf.jar;xstream-1.4.10.jar -Dcom.fs.starfarer.settings.paths.saves=..\\saves -Dcom.fs.starfarer.settings.paths.screenshots=..\\screenshots -Dcom.fs.starfarer.settings.paths.mods=..\\mods -Dcom.fs.starfarer.settings.paths.logs=. com.fs.starfarer.StarfarerLauncher");
      let mut args_raw = read_to_string(install_dir.join("vmparams")).await.map_err(|err| err.to_string())?;
      args_raw = JAVA_REGEX.replace(&args_raw, "").to_string();
      let args: Vec<&str> = args_raw.split_ascii_whitespace().collect();

      Command::new(install_dir.join("jre").join("bin").join("java.exe"))
        .current_dir(install_dir.join("starsector-core"))
        .args(["-DlaunchDirect=true", &format!("-DstartRes={}x{}", resolution.0, resolution.1), "-DstartFS=false", "-DstartSound=true"])
        .args(args)
        .spawn()
        .expect("Execute Starsector")
    } else {
      Command::new(install_dir.join("starsector.exe"))
        .current_dir(install_dir)
        .spawn()
        .expect("Execute Starsector")
    };

    child.wait_with_output().await.map_or_else(|err| Err(err.to_string()), |_| Ok(()))
  }
}

enum AppCommands {
  OpenSettings,
  UpdateModDescription(Arc<ModEntry>),
}

#[derive(Default)]
pub struct AppDelegate {
  settings_id: Option<WindowId>,
  root_id: Option<WindowId>,
}

impl Delegate<App> for AppDelegate {
  fn command(
    &mut self,
    ctx: &mut DelegateCtx,
    _target: Target,
    cmd: &Command,
    data: &mut App,
    _env: &Env,
  ) -> Handled {
    if cmd.is(App::SELECTOR) {
      match cmd.get_unchecked(App::SELECTOR) {
        AppCommands::OpenSettings => {
          let install_dir = lens!(App, settings)
            .then(lens!(settings::Settings, install_dir))
            .get(data);
          lens!(App, settings)
            .then(lens!(settings::Settings, install_dir_buf))
            .put(
              data,
              install_dir.map_or_else(|| "".to_string(), |p| p.to_string_lossy().to_string()),
            );

          let settings_window = WindowDesc::new(
            settings::Settings::ui_builder()
              .lens(App::settings)
              .on_change(|_, _old, data, _| {
                if let Err(err) = data.settings.save() {
                  eprintln!("{:?}", err)
                }
              }),
          )
          .window_size((800., 400.))
          .show_titlebar(false);

          self.settings_id = Some(settings_window.id);

          ctx.new_window(settings_window);
          return Handled::Yes;
        }
        AppCommands::UpdateModDescription(desc) => {
          data.active = Some(desc.clone());

          return Handled::Yes;
        }
      }
    } else if let Some(SettingsCommand::UpdateInstallDir(new_install_dir)) =
      cmd.get(settings::Settings::SELECTOR)
    {
      if data.settings.install_dir != Some(new_install_dir.clone()) || data.settings.dirty {
        data.settings.dirty = false;
        data.settings.install_dir_buf = new_install_dir.to_string_lossy().to_string();
        data.settings.install_dir = Some(new_install_dir.clone());

        if data.settings.save().is_err() {
          eprintln!("Failed to save settings")
        };

        data.mod_list.mods.clear();
        data.runtime.spawn(get_starsector_version(ctx.get_external_handle(), new_install_dir.clone()));
        data.runtime.spawn(ModList::parse_mod_folder(
          ctx.get_external_handle(),
          Some(new_install_dir.clone()),
        ));
      }
      return Handled::Yes;
    } else if let Some(entry) = cmd.get(ModList::AUTO_UPDATE) {
      data
        .runtime
        .spawn(installer::Payload::Download(entry.clone()).install(
          ctx.get_external_handle(),
          data.settings.install_dir.clone().unwrap(),
          data.mod_list.mods.values().map(|v| v.id.clone()).collect(),
        ));
    } else if let Some(()) = cmd.get(App::REFRESH) {
      if let Some(install_dir) = data.settings.install_dir.as_ref() {
        data.runtime.spawn(ModList::parse_mod_folder(
          ctx.get_external_handle(),
          Some(install_dir.clone()),
        ));
      }
    } else if let Some(res) = cmd.get(GET_INSTALLED_STARSECTOR) {
      App::mod_list.then(ModList::starsector_version).put(data, res.as_ref().ok().cloned());
    }

    Handled::No
  }

  fn window_removed(&mut self, id: WindowId, _data: &mut App, _env: &Env, ctx: &mut DelegateCtx) {
    if Some(id) == self.settings_id {
      self.settings_id = None;
    } else if Some(id) == self.root_id {
      ctx.submit_command(commands::QUIT_APP)
    }
  }

  fn event(
    &mut self,
    ctx: &mut DelegateCtx,
    window_id: WindowId,
    event: druid::Event,
    data: &mut App,
    _: &Env,
  ) -> Option<druid::Event> {
    if let druid::Event::WindowConnected = event {
      if self.root_id.is_none() {
        self.root_id = Some(window_id);
        if data.settings.dirty {
          ctx.submit_command(Settings::SELECTOR.with(SettingsCommand::UpdateInstallDir(
            data.settings.install_dir.clone().unwrap_or_default(),
          )));
        }
      }
    } else if let Event::KeyDown(KeyEvent {
      key: Key::Escape, ..
    }) = event
    {
      ctx.submit_command(App::DUMB_UNIVERSAL_ESCAPE)
    }

    Some(event)
  }
}

struct InstallController;

impl<W: Widget<App>> Controller<App, W> for InstallController {
  fn event(
    &mut self,
    child: &mut W,
    ctx: &mut EventCtx,
    event: &Event,
    data: &mut App,
    env: &druid::Env,
  ) {
    match event {
      Event::MouseDown(mouse_event) => {
        if mouse_event.button == druid::MouseButton::Left {
          ctx.set_active(true);
          ctx.request_paint();
        }
      }
      Event::MouseUp(mouse_event) => {
        if ctx.is_active() && mouse_event.button == druid::MouseButton::Left {
          ctx.set_active(false);
          if ctx.is_hot() {
            let ext_ctx = ctx.get_external_handle();
            let menu: Menu<App> = Menu::empty()
              .entry(MenuItem::new("From Archive(s)").on_activate(
                move |_ctx, data: &mut App, _| {
                  let ext_ctx = ext_ctx.clone();
                  data.runtime.spawn(async move {
                    let res = AsyncFileDialog::new()
                      .add_filter(
                        "Archives",
                        &["zip", "7z", "7zip", "rar", "rar4", "rar5", "tar"],
                      )
                      .pick_files()
                      .await;

                    ext_ctx.submit_command(App::OPEN_FILE, res, Target::Auto)
                  });
                },
              ))
              .entry(MenuItem::new("From Folder").on_activate({
                let ext_ctx = ctx.get_external_handle();
                move |_ctx, data: &mut App, _| {
                  data.runtime.spawn({
                    let ext_ctx = ext_ctx.clone();
                    async move {
                      let res = AsyncFileDialog::new().pick_folder().await;

                      ext_ctx.submit_command(App::OPEN_FOLDER, res, Target::Auto)
                    }
                  });
                }
              }));

            ctx.show_context_menu::<App>(menu, ctx.to_window(mouse_event.pos))
          }
          ctx.request_paint();
        }
      }
      _ => {}
    }

    child.event(ctx, event, data, env);
  }
}

struct ModListController;

impl<W: Widget<App>> Controller<App, W> for ModListController {
  fn event(&mut self, child: &mut W, ctx: &mut EventCtx, event: &Event, data: &mut App, env: &Env) {
    if let Event::Command(cmd) = event {
      if let Some((conflict, install_to, entry)) = cmd.get(ModList::OVERWRITE) {
        if let Some(install_dir) = &data.settings.install_dir {
          data.runtime.spawn(
            installer::Payload::Resumed(entry.clone(), install_to.clone(), conflict.clone())
              .install(
                ctx.get_external_handle(),
                install_dir.clone(),
                data.mod_list.mods.values().map(|v| v.id.clone()).collect(),
              ),
          );
        }
        ctx.is_handled();
      } else if let Some(payload) = cmd.get(installer::INSTALL) {
        match payload {
          ChannelMessage::Success(entry) => {
            data.mod_list.mods.insert(entry.id.clone(), entry.clone());
            ctx.children_changed();
            println!("Successfully installed {}", entry.id.clone())
          }
          ChannelMessage::Duplicate(conflict, to_install, entry) => {
            let widget = Flex::column()
              .with_child(
                h3("Overwrite existing?")
                  .center()
                  .padding(2.)
                  .expand_width()
                  .background(theme::BACKGROUND_LIGHT)
                  .controller(DragWindowController::default()),
              )
              .with_child(Label::new(format!(
                "Encountered conflict when trying to install {}",
                entry.id
              )))
              .with_child(Label::new(match conflict {
                StringOrPath::String(id) => format!("A mod with ID {} alread exists.", id),
                StringOrPath::Path(path) => format!(
                  "A folder already exists at the path {}.",
                  path.to_string_lossy()
                ),
              }))
              .with_child(Maybe::or_empty(
                || Label::wrapped("NOTE: A .git directory has been detected in the target directory. Are you sure this isn't being used for development?")
              ).lens(lens::Constant(data.settings.git_warn.then(|| {
                let maybe_path = match conflict {
                  StringOrPath::String(id) => data.mod_list.mods.get(id).and_then(|e| Some(&e.path)),
                  StringOrPath::Path(path) => Some(path),
                };

                maybe_path.and_then(|p| {
                  if p.join(".git").exists() {
                    Some(())
                  } else {
                    None
                  }
                })
              }).flatten())))
              .with_child(Label::new(format!(
                "Would you like to replace the existing {}?",
                if let StringOrPath::String(_) = conflict {
                  "mod"
                } else {
                  "folder"
                }
              )))
              .with_flex_spacer(1.)
              .with_child(
                Flex::row()
                  .with_flex_spacer(1.)
                  .with_child(Button::new("Overwrite").on_click({
                    let conflict = match conflict {
                      StringOrPath::String(id) => data.mod_list.mods.get(id).unwrap().path.clone(),
                      StringOrPath::Path(path) => path.clone(),
                    };
                    let to_install = to_install.clone();
                    let entry = entry.clone();
                    move |ctx, _, _| {
                      ctx.submit_command(commands::CLOSE_WINDOW);
                      ctx.submit_command(
                        ModList::OVERWRITE
                          .with((conflict.clone(), to_install.clone(), entry.clone()))
                          .to(Target::Global),
                      )
                    }
                  }))
                  .with_child(
                    Button::new("Cancel")
                      .on_click(|ctx, _, _| ctx.submit_command(commands::CLOSE_WINDOW)),
                  ),
              )
              .cross_axis_alignment(druid::widget::CrossAxisAlignment::Start);
  
            ctx.new_sub_window(
              WindowConfig::default().show_titlebar(false)
                .resizable(true)
                .window_size((500.0, 200.0)),
              widget,
              data.mod_list.clone(),
              env.clone(),
            );
          }
          ChannelMessage::Error(err) => {
            eprintln!("Failed to install {}", err);
          }
        }
      }
    } else if let Event::Notification(notif) = event {
      if let Some(entry) = notif.get(ModEntry::AUTO_UPDATE) {
        let widget = Flex::column()
          .with_child(
            h3("Auto-update?")
              .center()
              .padding(2.)
              .expand_width()
              .background(theme::BACKGROUND_LIGHT)
              .controller(DragWindowController::default()),
          )
          .with_child(Label::new(format!(
            "Would you like to automatically update {}?",
            entry.name
          )))
          .with_child(Label::new(format!("Installed version: {}", entry.version)))
          .with_child(Label::new(format!(
            "New version: {}",
            entry
              .remote_version
              .as_ref()
              .map(|v| v.version.to_string())
              .unwrap_or_else(|| String::from(
                "Error: failed to retrieve version, this shouldn't be possible."
              ))
          )))
          .with_child(Maybe::or_empty(
            || Label::wrapped("NOTE: A .git directory has been detected in the target directory. Are you sure this isn't being used for development?")
          ).lens(lens::Constant(data.settings.git_warn.then(|| {
            if entry.path.join(".git").exists() {
              Some(())
            } else {
              None
            }
          }).flatten())))
          .with_flex_spacer(1.)
          .with_child(
            Flex::row()
              .with_flex_spacer(1.)
              .with_child(Button::new("Update").on_click({
                let entry = entry.clone();
                move |ctx, _, _| {
                  ctx.submit_command(commands::CLOSE_WINDOW);
                  ctx.submit_command(ModList::AUTO_UPDATE.with(entry.clone()).to(Target::Global))
                }
              }))
              .with_child(
                Button::new("Cancel")
                  .on_click(|ctx, _, _| ctx.submit_command(commands::CLOSE_WINDOW)),
              ),
          )
          .cross_axis_alignment(druid::widget::CrossAxisAlignment::Start);

        ctx.new_sub_window(
          WindowConfig::default().show_titlebar(false)
            .resizable(true)
            .window_size((500.0, 200.0)),
          widget,
          data.mod_list.clone(),
          env.clone(),
        );
      }
    }

    child.event(ctx, event, data, env)
  }
}

struct AppController;

impl<W: Widget<App>> Controller<App, W> for AppController {
  fn event(&mut self, child: &mut W, ctx: &mut EventCtx, event: &Event, data: &mut App, env: &Env) {
    if let Event::Command(cmd) = event {
      if let Some(settings::SettingsCommand::SelectInstallDir) = cmd.get(Settings::SELECTOR) {
        let ext_ctx = ctx.get_external_handle();
        ctx.set_disabled(true);
        data.runtime.spawn(async move {
          let res = AsyncFileDialog::new().pick_folder().await;

          if let Some(handle) = res {
            ext_ctx.submit_command(
              Settings::SELECTOR,
              SettingsCommand::UpdateInstallDir(handle.path().to_path_buf()),
              Target::Auto,
            )
          } else {
            ext_ctx.submit_command(App::ENABLE, (), Target::Auto)
          }
        });
      } else if let Some(()) = cmd.get(App::DUMB_UNIVERSAL_ESCAPE) {
        ctx.set_focus(data.widget_id);
        ctx.resign_focus();
      }
      if (cmd.is(ModList::SUBMIT_ENTRY) || cmd.is(App::ENABLE)) && ctx.is_disabled() {
        ctx.set_disabled(false);
      } else if cmd.is(App::DISABLE) {
        ctx.set_disabled(true)
      }
    }

    child.event(ctx, event, data, env)
  }
}

#[derive(Clone, Data, Lens)]
struct IndyToggleState {
  state: bool,
}

impl ScopeTransfer for IndyToggleState {
  type In = bool;
  type State = bool;

  fn read_input(&self, _: &mut Self::State, _: &Self::In) {}

  fn write_back_input(&self, _: &Self::State, _: &mut Self::In) {}
}
