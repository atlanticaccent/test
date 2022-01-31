use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use druid::{Widget, widget::{Scroll, List, ListIter, Painter, Flex, Either, Label, Button, Controller}, lens, WidgetExt, Data, Lens, RenderContext, theme, Selector, ExtEventSink, Target, LensExt, WindowConfig, Env, commands};
use druid_widget_nursery::WidgetExt as WidgetExtNursery;
use if_chain::if_chain;
use serde::{Serialize, Deserialize};

use super::{mod_entry::ModEntry, util::{SaveError, self}, installer::{self, ChannelMessage, StringOrPath, HybridPath}};

pub mod headings;

#[derive(Clone, Data, Lens)]
pub struct ModList {
  #[data(same_fn="PartialEq::eq")]
  pub mods: BTreeMap<String, Arc<ModEntry>>,
}

impl ModList {
  const SUBMIT_ENTRY: Selector<Arc<ModEntry>> = Selector::new("mod_list.submit_entry");
  pub const OVERWRITE: Selector<(PathBuf, HybridPath, Arc<ModEntry>)> = Selector::new("mod_list.notification.overwrite");

  pub fn new() -> Self {
    Self {
      mods: BTreeMap::new(),
    }
  }

  pub fn ui_builder() -> impl Widget<Self> {
    Flex::column()
      .with_child(headings::Headings::ui_builder().lens(lens::Unit))
      .with_flex_child(
        Either::new(
          |data: &ModList, _| data.mods.len() > 0,
          Scroll::new(
            List::new(|| {
              ModEntry::ui_builder().expand_width().lens(lens!((Arc<ModEntry>, usize), 0)).background(Painter::new(|ctx, (_, i), env| {
                let rect = ctx.size().to_rect();
                if i % 2 == 0 {
                  ctx.fill(rect, &env.get(theme::BACKGROUND_DARK))
                } else {
                  ctx.fill(rect, &env.get(theme::BACKGROUND_LIGHT))
                }
              }))
            }).lens(lens::Identity).background(theme::BACKGROUND_LIGHT).on_command(ModEntry::REPLACE, |ctx, payload, data: &mut ModList| {
              data.mods.insert(payload.id.clone(), payload.clone());
              ctx.children_changed();
            }).controller(InstallController)
          ).vertical(),
          Label::new("No mods").expand().background(theme::BACKGROUND_LIGHT)
        ),
        1.
      )
      .on_command(ModList::SUBMIT_ENTRY, |_ctx, payload, data| {
        data.mods.insert(payload.id.clone(), payload.clone());
      })
      .on_command(util::MASTER_VERSION_RECEIVED, |_ctx, payload, data| {
        if let Ok(meta) = payload.1.clone() {
          if let Some(mut entry) = data.mods.get(&payload.0).cloned() {
            ModEntry::remote_version.in_arc().put(&mut entry, Some(meta));
            data.mods.insert(entry.id.clone(), entry);
          };
        }
      })
  }

  pub async fn parse_mod_folder(event_sink: ExtEventSink, root_dir: Option<PathBuf>) {
    if let Some(root_dir) = root_dir {
      let mod_dir = root_dir.join("mods");
      let enabled_mods_filename = mod_dir.join("enabled_mods.json");

      let enabled_mods = if !enabled_mods_filename.exists() {
        vec![]
      } else {
        if_chain! {
          if let Ok(enabled_mods_text) = std::fs::read_to_string(enabled_mods_filename);
          if let Ok(EnabledMods { enabled_mods }) = serde_json::from_str::<EnabledMods>(&enabled_mods_text);
          then {
            enabled_mods
          } else {
            return
          }
        }
      };

      if let Ok(dir_iter) = std::fs::read_dir(mod_dir) {
        let enabled_mods_iter = enabled_mods.iter();

        dir_iter
          .filter_map(|entry| entry.ok())
          .filter(|entry| {
            if let Ok(file_type) = entry.file_type() {
              file_type.is_dir()
            } else {
              false
            }
          })
          .filter_map(|entry| {
            if let Ok(mut mod_info) = ModEntry::from_file(&entry.path()) {
              mod_info.set_enabled(enabled_mods_iter.clone().find(|id| mod_info.id.clone().eq(*id)).is_some());
              Some((
                Arc::new(mod_info.clone()),
                mod_info.version_checker.clone()
              ))
            } else {
              dbg!(entry.path());
              None
            }
          })
          .for_each(|(entry, version)| {
            if let Err(err) = event_sink.submit_command(ModList::SUBMIT_ENTRY, entry, Target::Auto) {
              eprintln!("Failed to submit found mod {}", err);
            };
            if let Some(version) = version {
              tokio::spawn(util::get_master_version(event_sink.clone(), version));
            }
          });

        // self.mods.extend(mods);

        // versions.iter()
        //   .filter_map(|v| v.as_ref())
        //   .map(|v| Command::perform(util::get_master_version(v.clone()), ModListMessage::MasterVersionReceived))
        //   .collect()
      } else {
        // debug_println!("Fatal. Could not parse mods folder. Alert developer");
        
      }
    } else {
      
    }
  }
}

impl ListIter<(Arc<ModEntry>, usize)> for ModList {
  fn for_each(&self, mut cb: impl FnMut(&(Arc<ModEntry>, usize), usize)) {
    for (i, item) in self.mods.values().cloned().enumerate() {
      cb(&(item, i), i);
    }
  }
  
  fn for_each_mut(&mut self, mut cb: impl FnMut(&mut (Arc<ModEntry>, usize), usize)) {
    for (i, item) in self.mods.values_mut().enumerate() {
      cb(&mut (item.clone(), i), i);
    }
  }
  
  fn data_len(&self) -> usize {
    self.mods.len()
  }
}

struct InstallController;

impl<W: Widget<ModList>> Controller<ModList, W> for InstallController {
  fn event(&mut self, child: &mut W, ctx: &mut druid::EventCtx, event: &druid::Event, mod_list: &mut ModList, env: &Env) {
    if let druid::Event::Command(cmd) = event {
      if let Some(payload) = cmd.get(installer::INSTALL) {
        match payload {
          ChannelMessage::Success(entry) => {
            mod_list.mods.insert(entry.id.clone(), entry.clone());
            ctx.children_changed();
            println!("Successfully installed {}", entry.id.clone())
          },
          ChannelMessage::Duplicate(conflict, to_install, entry) => {
            let widget = Flex::column()
              .with_child(Label::new(format!("Encountered conflict when trying to install {}", entry.id)))
              .with_child(Label::new(match conflict {
                StringOrPath::String(id) => format!("A mod with ID {} alread exists.", id),
                StringOrPath::Path(path) => format!("A folder already exists at the path {}.", path.to_string_lossy()),
              }))
              .with_child(Label::new(format!("Would you like to replace the existing {}?", if let StringOrPath::String(_) = conflict { "mod" } else { "folder" })))
              .with_default_spacer()
              .with_child(
                Flex::row()
                  .with_child(Button::new("Overwrite").on_click({
                    let conflict = match conflict {
                      StringOrPath::String(id) => mod_list.mods.get(id).unwrap().path.clone(),
                      StringOrPath::Path(path) => path.clone(),
                    };
                    let to_install = to_install.clone();
                    let entry = entry.clone();
                    move |ctx, _, _| {
                      ctx.submit_command(commands::CLOSE_WINDOW);
                      ctx.submit_command(ModList::OVERWRITE.with((conflict.clone(), to_install.clone(), entry.clone())).to(Target::Global))
                    }
                  }))
                  .with_child(Button::new("Cancel").on_click(|ctx, _, _| {
                    ctx.submit_command(commands::CLOSE_WINDOW)
                  }))
              ).cross_axis_alignment(druid::widget::CrossAxisAlignment::Start);

            ctx.new_sub_window(
              WindowConfig::default().resizable(true).window_size((500.0, 200.0)),
              widget,
              mod_list.clone(),
              env.clone()
            );
          },
          ChannelMessage::Error(err) => {
            eprintln!("Failed to install {}", err);
          }
        }
      }
    }

    child.event(ctx, event, mod_list, env)
  }
}

#[derive(Serialize, Deserialize)]
pub struct EnabledMods {
  #[serde(rename = "enabledMods")]
  enabled_mods: Vec<String>
}

impl EnabledMods {
  pub fn save(self, path: &PathBuf) -> Result<(), SaveError> {
    use std::fs;
    use std::io::Write;

    let json = serde_json::to_string_pretty(&self)
      .map_err(|_| SaveError::FormatError)?;

    let mut file = fs::File::create(path.join("mods").join("enabled_mods.json"))
      .map_err(|_| SaveError::FileError)?;

    file.write_all(json.as_bytes())
      .map_err(|_| SaveError::WriteError)
  }
}

impl From<Vec<Arc<ModEntry>>> for EnabledMods {
  fn from(from: Vec<Arc<ModEntry>>) -> Self {
    Self {
      enabled_mods: from.iter().into_iter().map(|v| v.id.clone()).collect()
    }
  }
}
