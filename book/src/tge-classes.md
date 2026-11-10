# TGE classes not yet in torque-rs

torque-rs has about half of TGE 1.4's console classes as script classes. This page lists the 104 it has no script class for,
grouped by TGE's engine folders (the network events at the end are handled another way). "Partial" means torque-rs already has a piece of it (the network record, a reader, the Rust logic) but no
script class a game can create or call. Classes torque-rs replaced with its own design are listed at the end and are not
planned as they are.

The list is made from the class names in TGE 1.4's source and the [class reference](reference/script/classes.md); see the
[roadmap](roadmap.md) for the order things are planned in.

## Networking and console

| Class | What it is | In torque-rs |
|---|---|---|
| `TCPObject` | A script TCP client or listener with line callbacks | In progress |
| `HTTPObject` | HTTP `get` and `post` on top of `TCPObject` | In progress |
| `BanList` | Timed bans by address, checked on connect | |
| `ConsoleLogger` | Mirrors console output to a file with a level filter | Partial: the client's console log file |
| `LangTable` | Numbered translated strings for the GUI | |
| `SimChunk`, `TextChunk`, `UnknownChunk` | Structured chunk files | |
| `SimpleNetObject` | A minimal networked object with a message | Partial: the record is decoded |

## Game objects and AI

| Class | What it is | In torque-rs |
|---|---|---|
| `AIClient`, `AIConnection` | A bot as a client with no socket, with a move, aim and trigger API | AIPlayer's walk and aim logic is in `torque-sim`; no client object |
| `AIWheeledVehicle` | A bot-driven wheeled vehicle | Partial: the record is decoded |
| `HoverVehicle`, `HoverVehicleData` | Hover vehicles | Partial: the records are decoded; no simulation |
| `VehicleBlocker` | An invisible box only vehicles collide with | Partial: decoded and known to collision |
| `TSStatic` | A plain shape placed in a mission | Partial: the record is decoded |
| `ClientAudioEmitter` | The client side of an audio emitter | Partial: the record is decoded |
| `FireballAtmosphere`, `FireballAtmosphereData` | A rain of fireballs | Partial: the records are decoded |
| `DebugView` | Debug lines and text over the 3D view | Partial: the gizmos plugin draws debug shapes |

## Effects and weather

| Class | What it is | In torque-rs |
|---|---|---|
| `Precipitation`, `PrecipitationData` | Rain and snow | Partial: the records are decoded |
| `Lightning`, `LightningData` | Lightning strikes with a flash and thunder | Partial: the records are decoded |
| `WeatherLightning`, `WeatherLightningData` | Distant sheet lightning | Partial: the records are decoded |
| `fxLight`, `fxLightData` | Placed lights with animation | Partial: decoded and drawn; no script class |
| `fxSunLight` | The sun's flare and its setters | Partial: decoded and drawn; no script class |
| `fxShapeReplicatedStatic` | The shapes a shape replicator scatters | Partial: the record is decoded |
| `DecalManager` | The script object for bullet holes and marks | Partial: the client draws decals |

## World

| Class | What it is | In torque-rs |
|---|---|---|
| `PathedInterior`, `PathedInteriorData` | Doors, lifts and moving platforms made of interiors | Partial: the interior reader parses the paths |
| `InteriorMap` | Interiors from `.map` brush files | Partial: the record is decoded |
| `MirrorSubObject` | Mirrors in interiors | Partial: interior mirrors are drawn |
| `MaterialPropertyMap` | Maps a texture to footstep sounds, puffs and impacts | |
| `AudioSampleEnvironment` | Reverb settings for a sound | Partial: the datablock is decoded |
| `TSShapeConstructor` | A shape assembled from a base shape and sequence files | Partial: read and used when shapes load; no script class |

## GUI controls

| Class | What it is |
|---|---|
| `GuiTabBookCtrl`, `GuiTabPageCtrl` | Tabs |
| `GuiStackControl` | Children stacked in a row or column |
| `GuiPaneControl` | A collapsible pane |
| `GuiControlArrayControl` | Children in a grid of columns |
| `GuiFrameSetCtrl` | Resizable frames |
| `GuiTreeViewCtrl` | A tree |
| `GuiDirectoryTreeCtrl`, `GuiDirectoryFileListCtrl` | A folder tree and its file list |
| `GuiColorPickerCtrl` | A color picker |
| `GuiBackgroundCtrl`, `GuiBitmapBorderCtrl`, `GuiBorderButtonCtrl`, `GuiBitmapButtonTextCtrl`, `GuiBubbleTextCtrl`, `GuiNoMouseCtrl` | Small frames and buttons |
| `GuiTickCtrl` | A control that ticks with the game |
| `GuiClockHud`, `GuiSpeedometerHud` | A clock and a speedometer on the HUD |
| `GuiPlayerView` | A rotating view of a player model |
| `GuiVectorFieldCtrl` | Draws a vector field |
| `GuiAviBitmapCtrl`, `GuiTheoraCtrl` | Video playback |
| `GuiEffectCanvas` | A canvas with screen effects |

torque-rs has a flex and a grid layout of its own (`GuiFlexCtrl`, `GuiGridCtrl`) that cover much of what the stack and array
controls do.

## Editors and tools

These are development tools; torque-rs plans them as build-time features (see [Tools](roadmap.md#tools)).

| Class | What it is |
|---|---|
| `WorldEditor`, `EditTSCtrl`, `EditManager`, `CreatorTree` | The world editor and its object creator |
| `TerrainEditor`, `Terraformer`, `GuiTerrPreviewCtrl` | Terrain editing and generation |
| `MissionAreaEditor` | The mission area editor |
| `GuiEditCtrl`, `GuiEditorRuler`, `GuiControlListPopUp`, `GuiMenuBar`, `GuiFilterCtrl` | The GUI editor and its parts |
| `GuiInspector` and its field classes (`GuiInspectorField`, `GuiInspectorGroup`, `GuiInspectorDynamicField`, `GuiInspectorDatablockField`, `GuiInspectorTypeCheckBox`, `GuiInspectorTypeColor`, `GuiInspectorTypeColorF`, `GuiInspectorTypeColorI`, `GuiInspectorTypeEnum`, `GuiInspectorTypeFileName`, `GuiInspectorTypeGuiProfile`) | The object inspector |
| `DbgFileView` | The script debugger's source view |
| `ShowTSCtrl` | The shape viewer tool |

## Network events

TGE's network events are not script classes in torque-rs: the network layer carries them itself. `ConnectionMessageEvent`,
`GhostAlwaysObjectEvent`, `NetStringEvent`, `RemoteCommandEvent`, `SimDataBlockEvent`, `Sim2DAudioEvent`, `Sim3DAudioEvent`,
`SetMissionCRCEvent`, `SimpleMessageEvent`, `PathManagerEvent` and `LightningStrikeEvent` are read and written as TGE sends them.
`AudioEmitterToEvent` and `WeatherLightningStrikeEvent` are not handled yet. `FileChunkEvent` and `FileDownloadRequestEvent`
(TGE's file downloads) are replaced by torque-rs's [uploads](uploads.md) and mod downloads.
