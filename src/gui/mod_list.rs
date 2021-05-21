use std::{io, io::Read, path::PathBuf, collections::BTreeMap, fs::{read_dir, rename, remove_dir_all, create_dir_all, copy}};
use iced::{Text, Column, Command, Element, Length, Row, Scrollable, scrollable, Button, button, Checkbox, Container, Rule, PickList, pick_list, Space};
use serde::{Serialize, Deserialize};
use json_comments::strip_comments;
use json5;
use if_chain::if_chain;
use native_dialog::{FileDialog, MessageDialog, MessageType};

use crate::archive_handler;
use crate::style;
use crate::gui::SaveError;

pub struct ModList {
  root_dir: Option<PathBuf>,
  pub mods: BTreeMap<String, ModEntry>,
  scroll: scrollable::State,
  mod_description: ModDescription,
  install_state: pick_list::State<InstallOptions>,
  currently_highlighted: Option<String>
}

#[derive(Debug, Clone)]
pub enum ModListMessage {
  SetRoot(Option<PathBuf>),
  ModEntryMessage(String, ModEntryMessage),
  ModDescriptionMessage(ModDescriptionMessage),
  InstallPressed(InstallOptions),
  EnabledModsSaved(Result<(), SaveError>)
}

impl ModList {
  pub fn new() -> Self {
    ModList {
      root_dir: None,
      mods: BTreeMap::new(),
      scroll: scrollable::State::new(),
      mod_description: ModDescription::new(),
      install_state: pick_list::State::default(),
      currently_highlighted: None
    }
  }

  pub fn update(&mut self, message: ModListMessage) -> Command<ModListMessage> {
    match message {
      ModListMessage::SetRoot(root_dir) => {
        self.root_dir = root_dir;

        self.parse_mod_folder();

        return Command::none();
      },
      ModListMessage::ModEntryMessage(id, message) => {
        if let Some(entry) = self.mods.get_mut(&id) {
          match message {
            ModEntryMessage::EntryHighlighted => {
              self.mod_description.update(ModDescriptionMessage::ModChanged(entry.clone()));

              entry.update(ModEntryMessage::EntryHighlighted);

              if let Some(key) = &self.currently_highlighted {
                if !id.eq(key) {
                  let key = key.clone();
                  if let Some(old_entry) = self.mods.get_mut(&key) {
                    old_entry.update(ModEntryMessage::EntryCleared);
                  }
                }
              }

              self.currently_highlighted = Some(id);
            },
            ModEntryMessage::EntryCleared => {},
            ModEntryMessage::ToggleEnabled(_) => {
              entry.update(message);

              if let Some(path) = &self.root_dir {
                let enabled_mods = EnabledMods {
                  enabled_mods: self.mods.iter()
                    .filter_map(|(id, ModEntry { enabled, .. })| if *enabled {
                      Some(id.clone())
                    } else {
                      None 
                    })
                    .collect(),
                };
                return Command::perform(enabled_mods.save(path.join("mods").join("enabled_mods.json")), ModListMessage::EnabledModsSaved)
              }
            }
          }
        }

        Command::none()
      },
      ModListMessage::ModDescriptionMessage(message) => {
        self.mod_description.update(message);

        Command::none()
      },
      ModListMessage::InstallPressed(opt) => {
        if let Some(root_dir) = self.root_dir.clone() {
          let diag = FileDialog::new().set_location(&root_dir);

          match opt {
            InstallOptions::FromArchive => {
              let mut filters = vec!["zip", "rar"];
              if cfg!(unix) {
                filters.push("7z");
              }
              if let Ok(paths) = diag.add_filter("Archive types", &filters).show_open_multiple_file() {
                let res: Vec<String> = paths.iter()
                  .filter_map(|maybe_path| {
                    if_chain! {
                      if let Some(path) = maybe_path.to_str();
                      if let Some(_full_name) = maybe_path.file_name();
                      if let Some(_file_name) = maybe_path.file_stem();
                      if let Some(file_name) = _file_name.to_str();
                      let mod_dir = root_dir.join("mods");
                      let raw_temp_dest = mod_dir.join("temp");
                      let raw_dest = mod_dir.join(_file_name);
                      if let Some(temp_dest) = raw_temp_dest.to_str();
                      then {
                        match archive_handler::handle_archive(&path.to_owned(), &temp_dest.to_owned(), &file_name.to_owned()) {
                          Ok(true) => {
                            if raw_dest.exists() {
                              match ModList::make_query("A directory with this name already exists. Do you want to replace it?\nChoosing no will abort this operation.".to_string()) {
                                Ok(true) => {
                                  if remove_dir_all(&raw_dest).is_err() {
                                    return Some("Failed to delete existing mod directory. Aborting.".to_string())
                                  }
                                },
                                Ok(false) => return None,
                                Err(_) => return Some("Native dialog error.".to_string())
                              }
                            }
  
                            match ModList::find_nested_mod(&raw_temp_dest) {
                              Ok(Some(mod_path)) => {
                                if let Ok(_) = rename(mod_path, raw_dest) {
                                  if raw_temp_dest.exists() {
                                    if remove_dir_all(&raw_temp_dest).is_err() {
                                      return Some("Failed to clean up temporary directory. This is not a fatal error.".to_string())
                                    }
                                  }
                                  self.parse_mod_folder();
                                  None
                                } else {
                                  Some("Failed to move mod out of temporary directory. This may or may not be a fatal error.".to_string())
                                }
                              },
                              _ => Some("Could not find mod in given archive.".to_string())
                            }
                          },
                          Ok(false) => {
                            Some("Encountered unsupported feature.".to_string())
                          },
                          Err(err) => {
                            Some(format!("{:?}", err))
                          }
                        }
                      } else {
                        Some("Failed to parse file name.".to_string())
                      }
                    }
                  }).collect();

                match res.len() {
                  0 => {},
                  i if i < paths.len() => {
                    ModList::make_alert(format!("There were one or more errors when decompressing the given archives.\nErrors were as follows:\n{:?}", res));
                  },
                  _ => {
                    ModList::make_alert(format!("Encountered errors for all given archives.\nErrors were as follows:\n{:?}", res));
                  }
                };
              }

              Command::none()
            },
            InstallOptions::FromFolder => {
              match diag.show_open_single_dir() {
                Ok(Some(source_path)) => {
                  if_chain! {
                    if let Some(_file_name) = source_path.file_stem();
                    let mod_dir = root_dir.join("mods");
                    let raw_dest = mod_dir.join(_file_name);
                    then {
                      match ModList::find_nested_mod(&source_path) {
                        Ok(Some(mod_path)) => {
                          let cont = if raw_dest.exists() {
                            match ModList::make_query("A directory with this name already exists. Do you want to replace it?\nChoosing no will abort this operation.".to_string()) {
                              Ok(true) => remove_dir_all(&raw_dest).is_ok(),
                              _ => false
                            }
                          } else {
                            true
                          };
    
                          if cont {
                            if let Err(error) = ModList::copy_dir_recursive(&raw_dest, &mod_path) {
                              ModList::make_alert(format!("Failed to copy the given mod directory into the mods folder.\nError:{:?}", error));
                            } else {
                              remove_dir_all(mod_path);
                              self.parse_mod_folder();
                            }
                          }
                        }
                        _ => {}
                      }
                    } else {
                      ModList::make_alert("Experienced an error. Did not move given folder into mods directory.".to_owned());
                    }
                  }
                },
                Ok(None) => { ModList::make_alert("Experienced an error. Did not move given folder into mods directory.".to_owned()); },
                _ => {}
              }

              Command::none()
            },
            _ => Command::none()
          }
        } else {
          ModList::make_alert("No install directory set. Please set the Starsector install directory in Settings.".to_string());
          return Command::none();
        }
      },
      ModListMessage::EnabledModsSaved(res) => {
        match res {
          Err(err) => println!("{:?}", err),
          _ => {}
        }

        Command::none()
      }
    }
  }

  pub fn view(&mut self) -> Element<ModListMessage> {
    let mut every_other = true;
    let content = Column::new()
      .push::<Element<ModListMessage>>(PickList::new(
          &mut self.install_state,
          &InstallOptions::SHOW[..],
          Some(InstallOptions::Default),
          ModListMessage::InstallPressed
        ).into()
      )
      .push(Space::with_height(Length::Units(10)))
      .push(Column::new()
        .push(Row::new()
          .push(Space::with_width(Length::Units(5)))
          .push(Text::new("Enabled").width(Length::FillPortion(1)))
          .push(Space::with_width(Length::Units(5)))
          .push(Text::new("Name").width(Length::FillPortion(2)))
          .push(Space::with_width(Length::Units(5)))
          .push(Text::new("ID").width(Length::FillPortion(2)))
          .push(Space::with_width(Length::Units(5)))
          .push(Text::new("Author").width(Length::FillPortion(2)))
          .push(Space::with_width(Length::Units(5)))
          .push(Text::new("Mod Version").width(Length::FillPortion(2)))
          .push(Space::with_width(Length::Units(5)))
          .push(Text::new("Starsector Version").width(Length::FillPortion(2)))
          .height(Length::Shrink)
          .push(Space::with_width(Length::Units(10)))
        )
      )
      .push(Rule::horizontal(2).style(style::max_rule::Rule))
      .push(Scrollable::new(&mut self.scroll)
        .height(Length::FillPortion(2))
        .push(Row::new()
          .push::<Element<ModListMessage>>(if self.mods.len() > 0 {
            self.mods
              .iter_mut()
              .fold(Column::new(), |col, (id, entry)| {
                every_other = !every_other;
                let id_clone = id.clone();
                col.push(
                  entry.view(every_other).map(move |message| {
                    ModListMessage::ModEntryMessage(id_clone.clone(), message)
                  })
                )
              })
              .width(Length::Fill)
              .into()
          } else {
            Column::new()
              .width(Length::Fill)
              .height(Length::Units(200))
              .push(Text::new("No mods found") //change this to be more helpful
                .width(Length::Fill)
                .size(25)
                .color([0.7, 0.7, 0.7])
              )
              .into()
          })
          .push(Space::with_width(Length::Units(10)))
        )
      )
      .push(Rule::horizontal(30))
      .push(
        Container::new(self.mod_description.view().map(|message| {
          ModListMessage::ModDescriptionMessage(message)
        }))
        .height(Length::FillPortion(1))
        .width(Length::Fill)
      );

    Column::new()
      .push(content)
      .padding(5)
      .height(Length::Fill)
      .into()
  }

  fn parse_mod_folder(&mut self) {
    self.mods.clear();

    if_chain! {
      if let Some(root_dir) = &self.root_dir;
      let mod_dir = root_dir.join("mods");
      let enabled_mods_filename = mod_dir.join("enabled_mods.json");
      if let Ok(enabled_mods_text) = std::fs::read_to_string(enabled_mods_filename);
      if let Ok(EnabledMods { enabled_mods, .. }) = serde_json::from_str::<EnabledMods>(&enabled_mods_text);
      if let Ok(dir_iter) = std::fs::read_dir(mod_dir);
      then {
        let enabled_mods_iter = enabled_mods.iter();

        let mods = dir_iter
          .filter_map(|entry| entry.ok())
          .filter(|entry| {
            if let Ok(file_type) = entry.file_type() {
              file_type.is_dir()
            } else {
              false
            }
          })
          .filter_map(|entry| {
            let mod_info_path = entry.path().join("mod_info.json");
            if_chain! {
              if let Ok(mod_info_file) = std::fs::read_to_string(mod_info_path.clone());
              let mut stripped = String::new();
              if strip_comments(mod_info_file.as_bytes()).read_to_string(&mut stripped).is_ok();
              if let Ok(mut mod_info) = json5::from_str::<ModEntry>(&stripped);
              then {
                mod_info.enabled = enabled_mods_iter.clone().find(|id| mod_info.id.clone().eq(*id)).is_some();
                Some((
                  mod_info.id.clone(),
                  mod_info.clone()
                ))
              } else {
                None
              }
            }
          });

        self.mods.extend(mods)
      }
    }
  }

  pub fn make_alert(message: String) -> Result<(), String> {
    let mbox = move || {
      MessageDialog::new()
      .set_title("Alert:")
      .set_type(MessageType::Info)
      .set_text(&message)
      .show_alert()
      .map_err(|err| { err.to_string() })
    };

    // On windows we need to spawn a thread as the msg doesn't work otherwise
    #[cfg(target_os = "windows")]
    let res = match std::thread::spawn(move || {
      mbox()
    }).join() {
      Ok(Ok(())) => Ok(()),
      Ok(Err(err)) => Err(err),
      Err(err) => Err(err).map_err(|err| format!("{:?}", err))
    };

    #[cfg(not(target_os = "windows"))]
    let res = mbox();

    res
  }

  pub fn make_query(message: String) -> Result<bool, String> {
    let mbox = move || {
      MessageDialog::new()
      .set_type(MessageType::Warning)
      .set_text(&message)
      .show_confirm()
      .map_err(|err| { err.to_string() })
    };

    // On windows we need to spawn a thread as the msg doesn't work otherwise
    #[cfg(target_os = "windows")]
    let res = match std::thread::spawn(move || {
      mbox()
    }).join() {
      Ok(Ok(confirm)) => Ok(confirm),
      Ok(Err(err)) => Err(err),
      Err(err) => Err(err).map_err(|err| format!("{:?}", err))
    };

    #[cfg(not(target_os = "windows"))]
    let res = mbox();

    res
  }

  fn find_nested_mod(dest: &PathBuf) -> Result<Option<PathBuf>, io::Error> {
    for entry in read_dir(dest)? {
      let entry = entry?;
      if entry.file_type()?.is_dir() {
        let res = ModList::find_nested_mod(&entry.path())?;
        if res.is_some() { return Ok(res) }
      } else if entry.file_type()?.is_file() {
        if entry.file_name() == "mod_info.json" {
          return Ok(Some(dest.to_path_buf()));
        }
      }
    }

    Ok(None)
  }

  fn copy_dir_recursive(to: &PathBuf, from: &PathBuf) -> io::Result<()> {
    if !to.exists() {
      create_dir_all(to)?;
    }

    for entry in from.read_dir()? {
      let entry = entry?;
      if entry.file_type()?.is_dir() {
        ModList::copy_dir_recursive(&to.to_path_buf().join(entry.file_name()), &entry.path())?;
      } else if entry.file_type()?.is_file() {
        copy(entry.path(), &to.to_path_buf().join(entry.file_name()))?;
      }
    };

    Ok(())
  }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum InstallOptions {
  FromArchive,
  FromFolder,
  Default
}

impl InstallOptions {
  const SHOW: [InstallOptions; 2] = [
    InstallOptions::FromArchive,
    InstallOptions::FromFolder
  ];
}

impl std::fmt::Display for InstallOptions {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(
      f,
      "{}",
      match self {
        InstallOptions::Default => "Install Mod",
        InstallOptions::FromArchive => "From Archive",
        InstallOptions::FromFolder => "From Folder"
      }
    )
  }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModEntry {
  pub id: String,
  name: String,
  #[serde(default)]
  author: String,
  version: String,
  description: String,
  #[serde(alias = "gameVersion")]
  game_version: String,
  #[serde(skip)]
  enabled: bool,
  #[serde(skip)]
  highlighted: bool,
  #[serde(skip)]
  #[serde(default = "button::State::new")]
  button_state: button::State
}

#[derive(Debug, Clone)]
pub enum ModEntryMessage {
  ToggleEnabled(bool),
  EntryHighlighted,
  EntryCleared
}

impl ModEntry {
  pub fn update(&mut self, message: ModEntryMessage) -> Command<ModEntryMessage> {
    match message {
      ModEntryMessage::ToggleEnabled(enabled) => {
        self.enabled = enabled;

        Command::none()
      },
      ModEntryMessage::EntryHighlighted => {
        self.highlighted = true;

        Command::none()
      },
      ModEntryMessage::EntryCleared => {
        self.highlighted = false;

        Command::none()
      }
    }
  }

  pub fn view(&mut self, other: bool) -> Element<ModEntryMessage> {
    let row = Container::new(Row::new()
      .push(
        Checkbox::new(self.enabled, "", move |toggled| {
          ModEntryMessage::ToggleEnabled(toggled)
        })
        .width(Length::FillPortion(1))
      )
      .push(
        Button::new(
          &mut self.button_state,
          Row::new()
            .push(Rule::vertical(0).style(style::max_rule::Rule))
            .push(Space::with_width(Length::Units(5)))
            .push(Text::new(self.name.clone()).width(Length::Fill))
            .push(Rule::vertical(0).style(style::max_rule::Rule))
            .push(Space::with_width(Length::Units(5)))
            .push(Text::new(self.id.clone()).width(Length::Fill))
            .push(Rule::vertical(0).style(style::max_rule::Rule))
            .push(Space::with_width(Length::Units(5)))
            .push(Text::new(self.author.clone()).width(Length::Fill))
            .push(Rule::vertical(0).style(style::max_rule::Rule))
            .push(Space::with_width(Length::Units(5)))
            .push(Text::new(self.version.clone()).width(Length::Fill))
            .push(Rule::vertical(0).style(style::max_rule::Rule))
            .push(Space::with_width(Length::Units(5)))
            .push(Text::new(self.game_version.clone()).width(Length::Fill))
            .height(Length::Fill)
        )
        .padding(0)
        .height(Length::Fill)
        .style(style::button_none::Button)
        .on_press(ModEntryMessage::EntryHighlighted)
        .width(Length::FillPortion(10))
      )
      .height(Length::Units(50))
    );

    if self.highlighted {
      row.style(style::highlight_background::Container)
    } else if other {
      row.style(style::alternate_background::Container)
    } else {
      row
    }.into()
  }
}

#[derive(Debug, Clone)]
pub struct ModDescription {
  mod_entry: Option<ModEntry>
}

#[derive(Debug, Clone)]
pub enum ModDescriptionMessage {
  ModChanged(ModEntry)
}

impl ModDescription {
  pub fn new() -> Self {
    ModDescription {
      mod_entry: None
    }
  }

  pub fn update(&mut self, message: ModDescriptionMessage) -> Command<ModDescriptionMessage> {
    match message {
      ModDescriptionMessage::ModChanged(entry) => {
        self.mod_entry = Some(entry)
      }
    }

    Command::none()
  }

  pub fn view(&mut self) -> Element<ModDescriptionMessage> {
    Row::new()
      .push(Text::new(if let Some(entry) = &self.mod_entry {
        entry.description.clone()
      } else {
        "No mod selected.".to_owned()
      }))
      .padding(5)
      .into()
  }
}

#[derive(Serialize, Deserialize)]
pub struct EnabledMods {
  #[serde(rename = "enabledMods")]
  enabled_mods: Vec<String>
}

impl EnabledMods {
  pub async fn save(self, path: PathBuf) -> Result<(), SaveError> {
    use tokio::fs;
    use tokio::io::AsyncWriteExt;

    let json = serde_json::to_string_pretty(&self)
      .map_err(|_| SaveError::FormatError)?;

    let mut file = fs::File::create(path)
      .await
      .map_err(|_| SaveError::FileError)?;

    file.write_all(json.as_bytes())
      .await
      .map_err(|_| SaveError::WriteError)
  }
}
