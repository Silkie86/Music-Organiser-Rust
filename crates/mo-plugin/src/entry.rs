//! What the DAW finds in the plugin's file: its factories. The plugin factory lists every plugin in the file (for now
//! the one, "Music Organiser (test)"); another plugin in the same file goes in its list ([`PLUGINS`]), and another
//! kind of factory goes beside it in [`MoEntry`]. Beside it now: the preset discovery factory, whose providers list
//! your favourite sounds ([`crate::favourites`]) and your preset files with the app's parts ([`crate::preset_files`])
//! in the DAW's browser; another provider goes in its list ([`PROVIDERS`]).

use crate::MoPlugin;
use crate::{favourites, preset_files};
use clack_extensions::preset_discovery::prelude::*;
use clack_plugin::entry::prelude::*;
use clack_plugin::prelude::*;
use std::ffi::CStr;

/// How a plugin of the list is made, once the DAW asks for it.
type Make = for<'a> fn(HostInfo<'a>, &'a PluginDescriptor) -> PluginInstance<'a>;

/// Makes a plugin of type `P`.
fn make<'a, P: DefaultPluginFactory>(host: HostInfo<'a>, descriptor: &'a PluginDescriptor) -> PluginInstance<'a> {
    PluginInstance::new::<P>(host, descriptor, P::new_shared, P::new_main_thread)
}

/// Every plugin in the file: how it describes itself, and how it's made.
const PLUGINS: [(fn() -> PluginDescriptor, Make); 1] = [(MoPlugin::get_descriptor, make::<MoPlugin>)];

/// How a preset provider of the list is made, once the DAW asks for it.
type MakeProvider = for<'a> fn(IndexerInfo<'a>, &'a ProviderDescriptor) -> ProviderInstance<'a>;

/// Every preset provider in the file: how it describes itself, and how it's made.
const PROVIDERS: [(fn() -> ProviderDescriptor, MakeProvider); 2] =
    [(favourites::descriptor, favourites::provider), (preset_files::descriptor, preset_files::provider)];

/// The plugin file's entry: what the DAW asks for first.
pub struct MoEntry {
    plugins: PluginFactoryWrapper<Plugins>,
    presets: PresetDiscoveryFactoryWrapper<Providers>,
}

impl Entry for MoEntry {
    fn new(_bundle_path: Option<&CStr>) -> Result<Self, EntryLoadError> {
        let list = PLUGINS.iter().map(|(describe, make)| (describe(), *make)).collect();
        Ok(MoEntry {
            plugins: PluginFactoryWrapper::new(Plugins { list }),
            presets: PresetDiscoveryFactoryWrapper::new(Providers {
                list: PROVIDERS.iter().map(|(describe, make)| (describe(), *make)).collect(),
            }),
        })
    }

    fn declare_factories<'a>(&'a self, builder: &mut EntryFactories<'a>) {
        builder.register_factory(&self.plugins).register_factory(&self.presets);
    }
}

/// The plugin factory: the plugins in the file.
struct Plugins {
    list: Vec<(PluginDescriptor, Make)>,
}

impl PluginFactoryImpl for Plugins {
    fn plugin_count(&self) -> u32 {
        self.list.len() as u32
    }

    fn plugin_descriptor(&self, index: u32) -> Option<&PluginDescriptor> {
        self.list.get(index as usize).map(|(d, _)| d)
    }

    fn create_plugin<'a>(&'a self, host_info: HostInfo<'a>, plugin_id: &CStr) -> Option<PluginInstance<'a>> {
        let (descriptor, make) = self.list.iter().find(|(d, _)| d.id() == Some(plugin_id))?;
        Some(make(host_info, descriptor))
    }
}

/// The preset discovery factory: the preset providers in the file.
struct Providers {
    list: Vec<(ProviderDescriptor, MakeProvider)>,
}

impl PresetDiscoveryFactoryImpl for Providers {
    fn provider_count(&self) -> u32 {
        self.list.len() as u32
    }

    fn provider_descriptor(&self, index: u32) -> Option<&ProviderDescriptor> {
        self.list.get(index as usize).map(|(d, _)| d)
    }

    fn create_provider<'a>(&'a self, indexer: IndexerInfo<'a>, provider_id: &CStr) -> Option<ProviderInstance<'a>> {
        let (descriptor, make) = self.list.iter().find(|(d, _)| d.id() == Some(provider_id))?;
        Some(make(indexer, descriptor))
    }
}
