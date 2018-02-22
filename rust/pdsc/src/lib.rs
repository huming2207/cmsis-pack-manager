#[macro_use]
extern crate utils;
#[macro_use]
extern crate slog;
#[macro_use]
extern crate custom_derive;
#[macro_use]
extern crate enum_derive;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate failure;

extern crate pack_index;
extern crate clap;
extern crate minidom;

use std::borrow::Cow;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::collections::{HashMap, BTreeMap};
use minidom::{Element, Error, ErrorKind};
use clap::{App, Arg, ArgMatches, SubCommand};
use slog::Logger;

use utils::parse::{assert_root_name, attr_map, attr_parse, attr_parse_hex, child_text,
                   get_child_no_ns, FromElem};
use utils::ResultLogExt;
use pack_index::config::Config;
use failure::Error as FailError;

custom_derive!{
    #[allow(non_camel_case_types)]
    #[derive(Debug, PartialEq, Eq, EnumFromStr, Clone)]
    pub enum FileCategory{
        doc,
        header,
        include,
        library,
        object,
        source,
        sourceC,
        sourceCpp,
        sourceAsm,
        linkerScript,
        utility,
        image,
        other,
    }
}

custom_derive!{
    #[allow(non_camel_case_types)]
    #[derive(Debug, PartialEq, Eq, EnumFromStr, Clone)]
    pub enum FileAttribute{
        config, template
    }
}

#[derive(Debug, Clone)]
pub struct FileRef {
    path: PathBuf,
    category: FileCategory,
    attr: Option<FileAttribute>,
    condition: Option<String>,
    select: Option<String>,
    src: Option<String>,
    version: Option<String>,
}

impl FromElem for FileRef {
    fn from_elem(e: &Element, _: &Logger) -> Result<Self, Error> {
        assert_root_name(e, "file")?;
        Ok(Self {
            path: attr_map(e, "name", "file")?,
            category: attr_parse(e, "category", "file")?,
            attr: attr_parse(e, "attr", "file").ok(),
            condition: attr_map(e, "condition", "file").ok(),
            select: attr_map(e, "select", "file").ok(),
            src: attr_map(e, "src", "file").ok(),
            version: attr_map(e, "version", "file").ok(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ComponentBuilder {
    vendor: Option<String>,
    class: Option<String>,
    group: Option<String>,
    sub_group: Option<String>,
    variant: Option<String>,
    version: Option<String>,
    api_version: Option<String>,
    condition: Option<String>,
    max_instances: Option<u8>,
    is_default: bool,
    deprecated: bool,
    description: String,
    rte_addition: String,
    files: Vec<FileRef>,
}

impl FromElem for ComponentBuilder {
    fn from_elem(e: &Element, l: &Logger) -> Result<Self, Error> {
        assert_root_name(e, "component")?;
        let mut l = l.new(o!("in" => "Component"));
        let vendor: Option<String> = attr_map(e, "Cvendor", "component").ok();
        if let Some(v) = vendor.clone() {
            l = l.new(o!("Vendor" => v));
        }
        let class: Option<String> = attr_map(e, "Cclass", "component").ok();
        if let Some(c) = class.clone() {
            l = l.new(o!("Class" => c));
        }
        let group: Option<String> = attr_map(e, "Cgroup", "component").ok();
        if let Some(g) = group.clone() {
            l = l.new(o!("Group" => g));
        }
        let sub_group: Option<String> = attr_map(e, "Csub", "component").ok();
        if let Some(s) = vendor.clone() {
            l = l.new(o!("SubGroup" => s));
        }
        let files = e.get_child("files", "")
            .map(move |child| FileRef::vec_from_children(child.children(), &l))
            .unwrap_or_default();
        Ok(Self {
            vendor,
            class,
            group,
            sub_group,
            version: attr_map(e, "Cversion", "component").ok(),
            variant: attr_map(e, "Cvariant", "component").ok(),
            api_version: attr_map(e, "Capiversion", "component").ok(),
            condition: attr_map(e, "condition", "component").ok(),
            max_instances: attr_parse(e, "maxInstances", "component").ok(),
            is_default: attr_parse(e, "isDefaultVariant", "component").unwrap_or(true),
            description: child_text(e, "description", "component")?,
            deprecated: child_text(e, "deprecated", "component")
                .map(|s| s.parse().unwrap_or(false))
                .unwrap_or(false),
            rte_addition: child_text(e, "RTE_components_h", "component").unwrap_or_default(),
            files,
        })
    }
}

#[derive(Debug)]
pub struct Bundle {
    name: String,
    class: String,
    version: String,
    vendor: Option<String>,
    description: String,
    doc: String,
    components: Vec<ComponentBuilder>,
}

impl Bundle {
    pub fn into_components(self, l: &Logger) -> Vec<ComponentBuilder> {
        let class = self.class;
        let version = self.version;
        let vendor = self.vendor;
        if self.components.is_empty() {
            let mut l = l.new(o!("in" => "Bundle",
                                 "Class" => class.clone()));
            if let Some(v) = vendor.clone() {
                l = l.new(o!("Vendor" => v));
            }
            warn!(l, "Bundle should not be empty")
        }
        self.components
            .into_iter()
            .map(|comp| ComponentBuilder {
                class: comp.class.or_else(|| Some(class.clone())),
                version: comp.version.or_else(|| Some(version.clone())),
                vendor: comp.vendor.or_else(|| vendor.clone()),
                ..comp
            })
            .collect()
    }
}

impl FromElem for Bundle {
    fn from_elem(e: &Element, l: &Logger) -> Result<Self, Error> {
        assert_root_name(e, "bundle")?;
        let name: String = attr_map(e, "Cbundle", "bundle")?;
        let class: String = attr_map(e, "Cclass", "bundle")?;
        let version: String = attr_map(e, "Cversion", "bundle")?;
        let l = l.new(o!("Bundle" => name.clone(),
                         "Class" => class.clone(),
                         "Version" => version.clone()));
        let components = e.children()
            .filter_map(move |chld| {
                if chld.name() == "component" {
                    ComponentBuilder::from_elem(chld, &l).ok()
                } else {
                    None
                }
            })
            .collect();
        Ok(Self {
            name,
            class,
            version,
            vendor: attr_map(e, "Cvendor", "bundle").ok(),
            description: child_text(e, "description", "bundle")?,
            doc: child_text(e, "doc", "bundle")?,
            components,
        })
    }
}

fn child_to_component_iter(
    e: &Element,
    l: &Logger,
) -> Result<Box<Iterator<Item = ComponentBuilder>>, Error> {
    match e.name() {
        "bundle" => {
            let bundle = Bundle::from_elem(e, l)?;
            Ok(Box::new(bundle.into_components(l).into_iter()))
        }
        "component" => {
            let component = ComponentBuilder::from_elem(e, l)?;
            Ok(Box::new(Some(component).into_iter()))
        }
        _ => Err(Error::from_kind(ErrorKind::Msg(String::from(format!(
            "element of name {} is not allowed as a descendant of components",
            e.name()
        ))))),
    }
}

#[derive(Default)]
struct ComponentBuilders(Vec<ComponentBuilder>);

impl FromElem for ComponentBuilders {
    fn from_elem(e: &Element, l: &Logger) -> Result<Self, Error> {
        assert_root_name(e, "components")?;
        Ok(ComponentBuilders(e.children()
                .flat_map(move |c| match child_to_component_iter(c, l) {
                    Ok(iter) => iter,
                    Err(e) => {
                        error!(l, "when trying to parse component: {}", e);
                        Box::new(None.into_iter())
                    }
                })
                .collect()))
    }
}

struct ConditionComponent {
    pub device_family: Option<String>,
    pub device_sub_family: Option<String>,
    pub device_variant: Option<String>,
    pub device_vendor: Option<String>,
    pub device_name: Option<String>,
}

impl FromElem for ConditionComponent {
    fn from_elem(e: &Element, _: &Logger) -> Result<Self, Error> {
        Ok(ConditionComponent {
            device_family: attr_map(e, "Dfamily", "condition").ok(),
            device_sub_family: attr_map(e, "Dsubfamily", "condition").ok(),
            device_variant: attr_map(e, "Dvariant", "condition").ok(),
            device_vendor: attr_map(e, "Dvendor", "condition").ok(),
            device_name: attr_map(e, "Dname", "condition").ok(),
        })
    }
}

struct Condition {
    pub id: String,
    pub accept: Vec<ConditionComponent>,
    pub deny: Vec<ConditionComponent>,
    pub require: Vec<ConditionComponent>,
}

impl FromElem for Condition {
    fn from_elem(e: &Element, l: &Logger) -> Result<Self, Error> {
        assert_root_name(e, "condition")?;
        let mut accept = Vec::new();
        let mut deny = Vec::new();
        let mut require = Vec::new();
        for elem in e.children() {
            match elem.name() {
                "accept" => {
                    accept.push(ConditionComponent::from_elem(e, l)?);
                }
                "deny" => {
                    deny.push(ConditionComponent::from_elem(e, l)?);
                }
                "require" => {
                    require.push(ConditionComponent::from_elem(e, l)?);
                }
                "description" => {}
                _ => {
                    warn!(l, "Found unkonwn element {} in components", elem.name());
                }
            }
        }
        Ok(Condition {
            id: attr_map(e, "id", "condition")?,
            accept,
            deny,
            require,
        })
    }
}

#[derive(Default)]
struct Conditions(Vec<Condition>);

impl FromElem for Conditions {
    fn from_elem(e: &Element, l: &Logger) -> Result<Self, Error> {
        assert_root_name(e, "conditions")?;
        Ok(Conditions(
            e.children()
                .flat_map(|c| Condition::from_elem(c, l).ok_warn(l))
                .collect()))
    }
}

pub struct Release {
    pub version: String,
    pub text: String,
}

impl FromElem for Release {
    fn from_elem(e: &Element, _: &Logger) -> Result<Self, Error> {
        assert_root_name(e, "release")?;
        Ok(Self {
            version: attr_map(e, "version", "release")?,
            text: e.text(),
        })
    }
}

#[derive(Default)]
pub struct Releases(Vec<Release>);

impl Releases {
    pub fn latest_release(&self) -> &Release {
        &self.0[0]
    }
}

impl FromElem for Releases {
    fn from_elem(e: &Element, l: &Logger) -> Result<Self, Error> {
        assert_root_name(e, "releases")?;
        let to_ret: Vec<_> = e.children()
            .flat_map(|c| Release::from_elem(c, l).ok_warn(l))
            .collect();
        if to_ret.len() == 0usize {
            Err(err_msg!("There must be at least one release!"))
        } else {
            Ok(Releases(to_ret))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryPermissions {
    read: bool,
    write: bool,
    execute: bool,
}

impl MemoryPermissions {
    fn from_str(input: &str) -> Self {
        let mut ret = MemoryPermissions {
            read: false,
            write: false,
            execute: false,
        };
        for c in input.chars() {
            match c {
                'r' => ret.read = true,
                'w' => ret.write = true,
                'x' => ret.execute = true,
                _ => (),
            }
        }
        ret
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Memory {
    access: MemoryPermissions,
    start: u64,
    size: u64,
    startup: bool,
}

struct MemElem(String, Memory);

impl FromElem for MemElem {
    fn from_elem(e: &Element, _l: &Logger) -> Result<Self, Error> {
        let access = e.attr("id")
            .map(|memtype| {
                if memtype.contains("ROM") {
                    "rx"
                } else if memtype.contains("RAM") {
                    "rw"
                } else {
                    ""
                }
            })
            .or_else(|| e.attr("access"))
            .map(|memtype| MemoryPermissions::from_str(memtype))
            .unwrap();
        let name = e.attr("id")
            .or_else(|| e.attr("name"))
            .map(|s| s.to_string())
            .ok_or_else(|| err_msg!("No name found for memory"))?;
        let start = attr_parse_hex(e, "start", "memory")?;
        let size = attr_parse_hex(e, "size", "memory")?;
        let startup = attr_parse(e, "startup", "memory").unwrap_or_default();
        Ok(MemElem(
            name,
            Memory {
                access,
                start,
                size,
                startup,
            },
        ))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Memories(HashMap<String, Memory>);

fn merge_memories(lhs: Memories, rhs: &Memories) -> Memories {
    let rhs: Vec<_> = rhs.0.iter()
        .filter_map(|(k, v)| {
            if lhs.0.contains_key(k) {
                None
            } else {
                Some((k.clone(), v.clone()))
            }
        })
        .collect();
    let mut lhs = lhs;
    lhs.0.extend(rhs);
    lhs
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Algorithm{
    file_name: PathBuf,
    start: u64,
    size: u64,
    default: bool,
}

impl FromElem for Algorithm {
    fn from_elem(e: &Element, _l: &Logger) -> Result<Self, Error> {
        Ok(Self{
            file_name: attr_map(e, "name", "algorithm")?,
            start: attr_parse_hex(e, "start", "algorithm")?,
            size: attr_parse_hex(e, "size", "algorithm")?,
            default: attr_parse(e, "default", "algorithm").unwrap_or_default(),
        })
    }
}

#[derive(Debug)]
struct DeviceBuilder<'dom> {
    name: Option<&'dom str>,
    algorithms: Vec<Algorithm>,
    memories: Memories,
}

#[derive(Debug, Serialize)]
struct Device {
    name: String,
    memories: Memories,
    algorithms: Vec<Algorithm>,
}

impl<'dom> DeviceBuilder<'dom> {
    fn from_elem(e: &'dom Element) -> Self {
        let memories = Memories(HashMap::new());
        let bldr = DeviceBuilder {
            name: e.attr("Dname").or_else(|| e.attr("Dvariant")),
            memories,
            algorithms: Vec::new(),
        };
        bldr
    }

    fn build(self) -> Result<Device, Error> {
        Ok(Device {
            name: self.name
                .map(|s| s.into())
                .ok_or_else(|| err_msg!("Device found without a name"))?,
            memories: self.memories,
            algorithms: self.algorithms,
        })
    }

    fn add_parent(mut self, parent: &Self) -> Self {
        self.algorithms.extend_from_slice(&parent.algorithms);
        Self {
            name: self.name.or(parent.name),
            algorithms: self.algorithms,
            memories: merge_memories(self.memories, &parent.memories),
        }
    }

    fn add_memory(&mut self, MemElem(name, mem): MemElem) -> &mut Self {
        self.memories.0.insert(name, mem);
        self
    }

    fn add_algorithm(&mut self, alg: Algorithm) -> &mut Self {
        self.algorithms.push(alg);
        self
    }
}

fn parse_device<'dom>(e: &'dom Element, l: &Logger) -> Vec<DeviceBuilder<'dom>> {
    let mut device = DeviceBuilder::from_elem(e);
    let variants = e.children()
        .filter_map(|child| match child.name() {
            "variant" => Some(DeviceBuilder::from_elem(child)),
            "memory" => {
                FromElem::from_elem(child, l).map(|mem| device.add_memory(mem));
                None
            },
            "algorithm" => {
                FromElem::from_elem(child, l).map(|alg| device.add_algorithm(alg)).ok_warn(l);
                None
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if variants.is_empty() {
        vec![device]
    } else {
        variants
            .into_iter()
            .map(|bld| bld.add_parent(&device))
            .collect()
    }
}

fn parse_sub_family<'dom>(e: &'dom Element, l: &Logger) -> Vec<DeviceBuilder<'dom>> {
    let mut sub_family_device = DeviceBuilder::from_elem(e);
    let devices = e.children()
        .flat_map(|child| match child.name() {
            "device" => parse_device(child, l),
            "memory" => {
                FromElem::from_elem(child, l).map(|mem| sub_family_device.add_memory(mem));
                Vec::new()
            },
            "algorithm" => {
                FromElem::from_elem(child, l).map(|alg| sub_family_device.add_algorithm(alg));
                Vec::new()
            }
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    devices
        .into_iter()
        .map(|bldr| bldr.add_parent(&sub_family_device))
        .collect()
}

fn parse_family<'dom>(e: &Element, l: &Logger) -> Result<Vec<Device>, Error> {
    let mut family_device = DeviceBuilder::from_elem(e);
    let all_devices = e.children()
        .flat_map(|child| match child.name() {
            "subFamily" => parse_sub_family(child, &l),
            "device" => parse_device(child, &l),
            "memory" => {
                FromElem::from_elem(child, l).map(|mem| family_device.add_memory(mem));
                Vec::new()
            },
            "algorithm" => {
                FromElem::from_elem(child, l).map(|alg| family_device.add_algorithm(alg));
                Vec::new()
            }
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    all_devices
        .into_iter()
        .map(|bldr| bldr.add_parent(&family_device).build())
        .collect()
}

#[derive(Default, Serialize)]
struct Devices(HashMap<String, Device>);

impl FromElem for Devices {
    fn from_elem(e: &Element, l: &Logger) -> Result<Self, Error> {
        e.children()
            .fold(Ok(HashMap::new()), |res, c| match (res, parse_family(c, l)) {
                (Ok(mut devs), Ok(add_this)) => {
                    devs.extend(add_this.into_iter().map(|dev| (dev.name.clone(), dev)));
                    Ok(devs)
                },
                (Ok(_), Err(e)) => Err(e),
                (Err(e), Ok(_)) => Err(e),
                (Err(e), Err(_)) => Err(e),
            }).map(Devices)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct DumpDevice<'a> {
    name: &'a str,
    memories: Cow<'a, Memories>,
    algorithms: Cow<'a, Vec<Algorithm>>,
    from_pack: FromPack<'a>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FromPack<'a> {
    vendor: &'a str,
    pack: &'a str,
    version: &'a str,
}

impl<'a> FromPack<'a> {
    fn new(vendor: &'a str, pack: &'a str, version: &'a str) -> Self {
        Self{vendor, pack, version}
    }
}

impl<'a> DumpDevice<'a> {
    fn from_device(dev: &'a Device, from_pack: FromPack<'a>) -> Self {
        Self{
            name: &dev.name,
            memories: Cow::Borrowed(&dev.memories),
            algorithms: Cow::Borrowed(&dev.algorithms),
            from_pack: from_pack
        }
    }
}

pub struct Package {
    pub name: String,
    pub description: String,
    pub vendor: String,
    pub url: String,
    pub license: Option<String>,
    components: ComponentBuilders,
    pub releases: Releases,
    conditions: Conditions,
    devices: Devices,
    pub boards: Vec<Board>,
}

impl FromElem for Package {
    fn from_elem(e: &Element, l: &Logger) -> Result<Self, Error> {
        assert_root_name(e, "package")?;
        let name: String = child_text(e, "name", "package")?;
        let description: String = child_text(e, "description", "package")?;
        let vendor: String = child_text(e, "vendor", "package")?;
        let url: String = child_text(e, "url", "package")?;
        let l = l.new(o!("Vendor" => vendor.clone(),
                         "Package" => name.clone()
        ));
        let components = get_child_no_ns(e, "components")
            .and_then(|c| ComponentBuilders::from_elem(c, &l).ok_warn(&l))
            .unwrap_or_default();
        let releases = get_child_no_ns(e, "releases")
            .and_then(|c| Releases::from_elem(c, &l).ok_warn(&l))
            .unwrap_or_default();
        let conditions = get_child_no_ns(e, "conditions")
            .and_then(|c| Conditions::from_elem(c, &l).ok_warn(&l))
            .unwrap_or_default();
        let devices = get_child_no_ns(e, "devices")
            .and_then(|c| Devices::from_elem(c, &l).ok_warn(&l))
            .unwrap_or_default();
        let boards = get_child_no_ns(e, "boards")
            .map(|c| Board::vec_from_children(c.children(), &l))
            .unwrap_or_default();
        Ok(Self {
            name,
            description,
            vendor,
            url,
            components,
            license: child_text(e, "license", "package").ok(),
            releases,
            conditions,
            devices,
            boards,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct Board {
    name: String,
    mounted_devices: Vec<String>
}

impl FromElem for Board {
    fn from_elem(e: &Element, l: &Logger) -> Result<Self, Error> {
        Ok(Self{
            name: attr_map(e, "name", "board")?,
            mounted_devices: e.children().flat_map(|c| match c.name() {
                "mountedDevice" => {
                    attr_map(c, "Dname", "mountedDevice").ok()
                },
                _ => None
            }).collect()
        })
    }
}

#[derive(Debug)]
pub struct Component {
    vendor: String,
    class: String,
    group: String,
    sub_group: Option<String>,
    variant: Option<String>,
    version: String,
    api_version: Option<String>,
    condition: Option<String>,
    max_instances: Option<u8>,
    is_default: bool,
    deprecated: bool,
    description: String,
    rte_addition: String,
    files: Vec<FileRef>,
}

type Components = Vec<Component>;

impl Package {
    fn make_components(&self) -> Components {
        self.components.0
            .clone()
            .into_iter()
            .map(|comp| Component {
                vendor: comp.vendor.unwrap_or_else(|| self.vendor.clone()),
                class: comp.class.unwrap(),
                group: comp.group.unwrap(),
                sub_group: comp.sub_group,
                variant: comp.variant,
                version: comp.version
                    .unwrap_or_else(|| self.releases.latest_release().version.clone()),
                api_version: comp.api_version,
                condition: comp.condition,
                max_instances: comp.max_instances,
                is_default: comp.is_default,
                deprecated: comp.deprecated,
                description: comp.description,
                rte_addition: comp.rte_addition,
                files: comp.files,
            })
            .collect()
    }

    fn make_condition_lookup<'a>(&'a self, l: &Logger) -> HashMap<&'a str, &'a Condition> {
        let mut map = HashMap::with_capacity(self.conditions.0.iter().count());
        for cond in self.conditions.0.iter() {
            if let Some(dup) = map.insert(cond.id.as_str(), cond) {
                warn!(l, "Duplicate Condition found {}", dup.id);
            }
        }
        map
    }

    fn make_dump_devices<'a>(&'a self) -> Vec<(&'a str, DumpDevice<'a>)> {
        let from_pack = FromPack::new(&self.vendor, &self.name, &self.releases.latest_release().version);
        self.devices.0
            .iter()
            .map(|(name, d)| (name.as_str(), DumpDevice::from_device(d, from_pack.clone())))
            .collect()

    }
}

pub fn check_args<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("check")
        .about("Check a project or pack for correct usage of the CMSIS standard")
        .version("0.1.0")
        .arg(
            Arg::with_name("INPUT")
                .help("Input file to check")
                .required(true)
                .index(1),
        )
}

pub fn check_command<'a>(_: &Config, args: &ArgMatches<'a>, l: &Logger) -> Result<(), FailError> {
    let filename = args.value_of("INPUT").unwrap();
    match Package::from_path(Path::new(filename.clone()), &l) {
        Ok(c) => {
            info!(l, "Parsing succedded");
            info!(l, "{} Valid Conditions", c.conditions.0.iter().count());
            let cond_lookup = c.make_condition_lookup(l);
            let mut num_components = 0;
            let mut num_files = 0;
            for &Component {
                ref class,
                ref group,
                ref condition,
                ref files,
                ..
            } in c.make_components().iter()
            {
                num_components += 1;
                num_files += files.iter().count();
                if let &Some(ref cond_name) = condition {
                    if cond_lookup.get(cond_name.as_str()).is_none() {
                        warn!(
                            l,
                            "Component {}::{} references an unknown condition '{}'",
                            class,
                            group,
                            cond_name
                        );
                    }
                }
                for &FileRef {
                    ref path,
                    ref condition,
                    ..
                } in files.iter()
                {
                    if let &Some(ref cond_name) = condition {
                        if cond_lookup.get(cond_name.as_str()).is_none() {
                            warn!(
                                l,
                                "File {:?} Component {}::{} references an unknown condition '{}'",
                                path,
                                class,
                                group,
                                cond_name
                            );
                        }
                    }
                }
            }
            info!(l, "{} Valid Devices", c.devices.0.len());
            info!(l, "{} Valid Software Components", num_components);
            info!(l, "{} Valid Files References", num_files);
        }
        Err(e) => {
            error!(l, "parsing {}: {}", filename, e);
        }
    }
    debug!(l, "exiting");
    Ok(())
}

pub fn dump_devices_args<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("dump-devices")
        .about("Dump devices as json")
        .version("0.1.0")
        .arg(
            Arg::with_name("devices")
                .short("d")
                .takes_value(true)
                .help("Dump JSON in the specified file")
        )
        .arg(
            Arg::with_name("boards")
                .short("b")
                .takes_value(true)
                .help("Dump JSON in the specified file")
        )
        .arg(
            Arg::with_name("INPUT")
                .help("Input file to dump devices from")
                .index(1),
        )

}

pub fn dump_devices<'a, P: AsRef<Path>, I: IntoIterator<Item = &'a Package>>(
    pdscs: I,
    device_dest: Option<P>,
    board_dest: Option<P>,
    l: &Logger
) -> Result<(), FailError> {
    let pdscs: Vec<&Package> = pdscs.into_iter().collect();
    let devices = pdscs
        .iter()
        .flat_map(|pdsc| pdsc.make_dump_devices().into_iter())
        .collect::<HashMap<_, _>>();
    match device_dest {
        Some(to_file) =>  {
            if ! devices.is_empty() {
                let mut file_contents = Vec::new();
                let mut old_devices: HashMap<&str, DumpDevice> = HashMap::new();
                let mut all_devices = BTreeMap::new();
                if let Ok(mut fd) = OpenOptions::new().read(true).open(to_file.as_ref()) {
                    fd.read_to_end(&mut file_contents)?;
                    old_devices = serde_json::from_slice(&file_contents).unwrap_or_default();
                }
                all_devices.extend(old_devices.iter());
                all_devices.extend(devices.iter());
                let mut options =  OpenOptions::new();
                options.write(true);
                options.create(true);
                options.truncate(true);
                if let Ok(fd) = options.open(to_file.as_ref()) {
                    serde_json::to_writer_pretty(fd, &all_devices).unwrap();
                } else {
                    println!("Could not open file {:?}", to_file.as_ref());
                }
            }
        },
        None => println!("{}", &serde_json::to_string_pretty(&devices).unwrap()),
    }
    let boards = pdscs
        .iter()
        .flat_map(|pdsc| pdsc.boards.iter())
        .map(|b| (&b.name, b))
        .collect::<HashMap<_, _>>();
    match board_dest {
        Some(to_file) =>  {
            let mut options =  OpenOptions::new();
            options.write(true);
            options.create(true);
            options.truncate(true);
            if let Ok(fd) = options.open(to_file.as_ref()) {
                serde_json::to_writer_pretty(fd, &boards).unwrap();
            } else {
                println!("Could not open file {:?}", to_file.as_ref());
            }
        },
        None => println!("{}", &serde_json::to_string_pretty(&devices).unwrap()),
    }
    Ok(())
}

pub fn dump_devices_command<'a>(c: &Config, args: &ArgMatches<'a>, l: &Logger) -> Result<(), FailError> {
    let files = args.value_of("INPUT").map(|input| vec![Box::new(Path::new(input)).to_path_buf()]);
    let filenames = files.or_else(||{
        c.pack_store.read_dir().ok().map(
            |rd| rd.flat_map(
                |dirent| dirent.into_iter().map(|p| p.path())
            ).collect()
        )
    }).unwrap();
    let pdscs = filenames
        .into_iter()
        .flat_map(|filename|
                  match Package::from_path(&filename, &l) {
                      Ok(c) => Some(c),
                      Err(e) => {
                          error!(l, "parsing {:?}: {}", filename, e);
                          None
                      }
                  }
        ).collect::<Vec<Package>>();
    let to_ret = dump_devices(&pdscs, args.value_of("devices"), args.value_of("boards"), l);
    debug!(l, "exiting");
    to_ret
}