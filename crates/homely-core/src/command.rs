//! Command pattern and undo/redo transaction history for [`Project`].

use crate::{CoreError, FurnitureItem, Level, Opening, Project, Wall, WallAnchorPoint};
use serde::{Deserialize, Serialize};

/// Reversible mutation command applied to a [`Project`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProjectCommand {
    AddLevel(Level),
    RemoveLevel {
        level_id: String,
    },
    AddWall(Wall),
    RemoveWall {
        wall_id: String,
    },
    UpdateWallAnchor {
        wall_id: String,
        anchor_index: usize,
        anchor: WallAnchorPoint,
    },
    InsertWallAnchor {
        wall_id: String,
        anchor_index: usize,
        anchor: WallAnchorPoint,
    },
    RemoveWallAnchor {
        wall_id: String,
        anchor_index: usize,
    },
    UpdateWallProperties {
        wall_id: String,
        thickness: f64,
        start_height: f64,
        end_height: f64,
        is_closed_loop: bool,
    },
    AddOpening(Opening),
    RemoveOpening {
        opening_id: String,
    },
    PlaceFurniture(FurnitureItem),
    RemoveFurniture {
        item_id: String,
    },
    Batch(Vec<ProjectCommand>),
}

impl ProjectCommand {
    /// Human-readable short description for UI history panels.
    pub fn label(&self) -> String {
        match self {
            Self::AddLevel(l) => format!("Add Level '{}'", l.name),
            Self::RemoveLevel { level_id } => format!("Remove Level {level_id}"),
            Self::AddWall(w) => format!("Add Wall {}", w.id),
            Self::RemoveWall { wall_id } => format!("Remove Wall {wall_id}"),
            Self::UpdateWallAnchor {
                wall_id,
                anchor_index,
                ..
            } => format!("Move {wall_id} #{anchor_index}"),
            Self::InsertWallAnchor {
                wall_id,
                anchor_index,
                ..
            } => format!("Insert {wall_id} #{anchor_index}"),
            Self::RemoveWallAnchor {
                wall_id,
                anchor_index,
            } => format!("Remove {wall_id} #{anchor_index}"),
            Self::UpdateWallProperties { wall_id, .. } => format!("Edit Wall {wall_id}"),
            Self::AddOpening(o) => format!("Add Opening {}", o.id),
            Self::RemoveOpening { opening_id } => format!("Remove Opening {opening_id}"),
            Self::PlaceFurniture(f) => format!("Place {}", f.catalog_item_id),
            Self::RemoveFurniture { item_id } => format!("Remove {item_id}"),
            Self::Batch(cmds) => format!("Shared Corner ({} walls)", cmds.len()),
        }
    }

    /// Applies the command to `project` and returns its exact inverse command.
    pub fn apply(&self, project: &mut Project) -> Result<Self, CoreError> {
        match self {
            Self::AddLevel(level) => {
                ensure_unique(&project.levels, &level.id, |l| &l.id, "level")?;
                project.levels.push(level.clone());
                Ok(Self::RemoveLevel {
                    level_id: level.id.clone(),
                })
            }
            Self::RemoveLevel { level_id } => {
                let idx = find_index(&project.levels, level_id, |l| &l.id, "level")?;
                let removed = project.levels.remove(idx);
                Ok(Self::AddLevel(removed))
            }
            Self::AddWall(wall) => {
                ensure_exists(&project.levels, &wall.level_id, |l| &l.id, "level")?;
                ensure_unique(&project.walls, &wall.id, |w| &w.id, "wall")?;
                project.walls.push(wall.clone());
                Ok(Self::RemoveWall {
                    wall_id: wall.id.clone(),
                })
            }
            Self::RemoveWall { wall_id } => {
                let idx = find_index(&project.walls, wall_id, |w| &w.id, "wall")?;
                let removed = project.walls.remove(idx);
                Ok(Self::AddWall(removed))
            }
            Self::UpdateWallAnchor {
                wall_id,
                anchor_index,
                anchor,
            } => {
                let wall = project
                    .walls
                    .iter_mut()
                    .find(|w| w.id == *wall_id)
                    .ok_or_else(|| CoreError::NotFound {
                        kind: "wall",
                        id: wall_id.clone(),
                    })?;
                let slot =
                    wall.anchors
                        .get_mut(*anchor_index)
                        .ok_or_else(|| CoreError::InvalidIndex {
                            kind: "wall_anchor",
                            id: wall_id.clone(),
                            index: *anchor_index,
                        })?;
                let previous = std::mem::replace(slot, anchor.clone());
                Ok(Self::UpdateWallAnchor {
                    wall_id: wall_id.clone(),
                    anchor_index: *anchor_index,
                    anchor: previous,
                })
            }
            Self::InsertWallAnchor {
                wall_id,
                anchor_index,
                anchor,
            } => {
                let wall = project
                    .walls
                    .iter_mut()
                    .find(|w| w.id == *wall_id)
                    .ok_or_else(|| CoreError::NotFound {
                        kind: "wall",
                        id: wall_id.clone(),
                    })?;
                if *anchor_index > wall.anchors.len() {
                    return Err(CoreError::InvalidIndex {
                        kind: "wall_anchor",
                        id: wall_id.clone(),
                        index: *anchor_index,
                    });
                }
                wall.anchors.insert(*anchor_index, anchor.clone());
                Ok(Self::RemoveWallAnchor {
                    wall_id: wall_id.clone(),
                    anchor_index: *anchor_index,
                })
            }
            Self::RemoveWallAnchor {
                wall_id,
                anchor_index,
            } => {
                let wall = project
                    .walls
                    .iter_mut()
                    .find(|w| w.id == *wall_id)
                    .ok_or_else(|| CoreError::NotFound {
                        kind: "wall",
                        id: wall_id.clone(),
                    })?;
                if *anchor_index >= wall.anchors.len() {
                    return Err(CoreError::InvalidIndex {
                        kind: "wall_anchor",
                        id: wall_id.clone(),
                        index: *anchor_index,
                    });
                }
                let removed = wall.anchors.remove(*anchor_index);
                Ok(Self::InsertWallAnchor {
                    wall_id: wall_id.clone(),
                    anchor_index: *anchor_index,
                    anchor: removed,
                })
            }
            Self::UpdateWallProperties {
                wall_id,
                thickness,
                start_height,
                end_height,
                is_closed_loop,
            } => {
                let wall = project
                    .walls
                    .iter_mut()
                    .find(|w| w.id == *wall_id)
                    .ok_or_else(|| CoreError::NotFound {
                        kind: "wall",
                        id: wall_id.clone(),
                    })?;
                let prev = Self::UpdateWallProperties {
                    wall_id: wall_id.clone(),
                    thickness: wall.thickness,
                    start_height: wall.start_height,
                    end_height: wall.end_height,
                    is_closed_loop: wall.is_closed_loop,
                };
                wall.thickness = *thickness;
                wall.start_height = *start_height;
                wall.end_height = *end_height;
                wall.is_closed_loop = *is_closed_loop;
                Ok(prev)
            }
            Self::AddOpening(opening) => {
                ensure_exists(&project.walls, &opening.wall_id, |w| &w.id, "wall")?;
                ensure_unique(&project.openings, &opening.id, |o| &o.id, "opening")?;
                project.openings.push(opening.clone());
                Ok(Self::RemoveOpening {
                    opening_id: opening.id.clone(),
                })
            }
            Self::RemoveOpening { opening_id } => {
                let idx = find_index(&project.openings, opening_id, |o| &o.id, "opening")?;
                let removed = project.openings.remove(idx);
                Ok(Self::AddOpening(removed))
            }
            Self::PlaceFurniture(item) => {
                ensure_exists(&project.levels, &item.level_id, |l| &l.id, "level")?;
                ensure_unique(&project.furniture, &item.id, |f| &f.id, "furniture")?;
                project.furniture.push(item.clone());
                Ok(Self::RemoveFurniture {
                    item_id: item.id.clone(),
                })
            }
            Self::RemoveFurniture { item_id } => {
                let idx = find_index(&project.furniture, item_id, |f| &f.id, "furniture")?;
                let removed = project.furniture.remove(idx);
                Ok(Self::PlaceFurniture(removed))
            }
            Self::Batch(cmds) => {
                let mut inverses = Vec::with_capacity(cmds.len());
                for cmd in cmds {
                    inverses.push(cmd.apply(project)?);
                }
                inverses.reverse();
                Ok(Self::Batch(inverses))
            }
        }
    }
}

fn ensure_unique<T>(
    items: &[T],
    id: &str,
    get_id: impl Fn(&T) -> &str,
    kind: &'static str,
) -> Result<(), CoreError> {
    if items.iter().any(|item| get_id(item) == id) {
        return Err(CoreError::DuplicateId {
            kind,
            id: id.to_string(),
        });
    }
    Ok(())
}

fn ensure_exists<T>(
    items: &[T],
    id: &str,
    get_id: impl Fn(&T) -> &str,
    kind: &'static str,
) -> Result<(), CoreError> {
    if !items.iter().any(|item| get_id(item) == id) {
        return Err(CoreError::NotFound {
            kind,
            id: id.to_string(),
        });
    }
    Ok(())
}

fn find_index<T>(
    items: &[T],
    id: &str,
    get_id: impl Fn(&T) -> &str,
    kind: &'static str,
) -> Result<usize, CoreError> {
    items
        .iter()
        .position(|item| get_id(item) == id)
        .ok_or_else(|| CoreError::NotFound {
            kind,
            id: id.to_string(),
        })
}

/// Linear undo/redo stack for [`ProjectCommand`]s.
#[derive(Debug, Default)]
pub struct CommandHistory {
    undo_stack: Vec<ProjectCommand>,
    redo_stack: Vec<ProjectCommand>,
}

impl CommandHistory {
    /// Executes a command, records its inverse, and clears the redo stack.
    pub fn execute(
        &mut self,
        project: &mut Project,
        cmd: ProjectCommand,
    ) -> Result<(), CoreError> {
        let inverse = cmd.apply(project)?;
        self.undo_stack.push(inverse);
        self.redo_stack.clear();
        Ok(())
    }

    /// Undoes the most recent command, returning `true` if a command was undone.
    pub fn undo(&mut self, project: &mut Project) -> Result<bool, CoreError> {
        let Some(inverse) = self.undo_stack.pop() else {
            return Ok(false);
        };
        let redo_cmd = inverse.apply(project)?;
        self.redo_stack.push(redo_cmd);
        Ok(true)
    }

    /// Redoes the most recently undone command, returning `true` if a command was redone.
    pub fn redo(&mut self, project: &mut Project) -> Result<bool, CoreError> {
        let Some(cmd) = self.redo_stack.pop() else {
            return Ok(false);
        };
        let inverse = cmd.apply(project)?;
        self.undo_stack.push(inverse);
        Ok(true)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    /// Labels of executed steps from oldest to newest.
    /// Note: `undo_stack` stores inverses, so we map the inverse label cleanly.
    pub fn undo_labels(&self) -> Vec<String> {
        self.undo_stack.iter().map(inverse_action_label).collect()
    }

    /// Labels of undone steps from next-to-redo to oldest.
    pub fn redo_labels(&self) -> Vec<String> {
        self.redo_stack.iter().rev().map(ProjectCommand::label).collect()
    }

    /// Jumps to a specific `target_undo_len` by undoing or redoing steps.
    pub fn jump_to(&mut self, project: &mut Project, target_undo_len: usize) -> Result<(), CoreError> {
        while self.undo_stack.len() > target_undo_len {
            if !self.undo(project)? {
                break;
            }
        }
        while self.undo_stack.len() < target_undo_len {
            if !self.redo(project)? {
                break;
            }
        }
        Ok(())
    }
}

fn inverse_action_label(inverse: &ProjectCommand) -> String {
    match inverse {
        ProjectCommand::RemoveLevel { level_id } => format!("Add Level {level_id}"),
        ProjectCommand::AddLevel(l) => format!("Remove Level '{}'", l.name),
        ProjectCommand::RemoveWall { wall_id } => format!("Add Wall {wall_id}"),
        ProjectCommand::AddWall(w) => format!("Remove Wall {}", w.id),
        ProjectCommand::RemoveWallAnchor {
            wall_id,
            anchor_index,
        } => format!("Insert {wall_id} #{anchor_index}"),
        ProjectCommand::InsertWallAnchor {
            wall_id,
            anchor_index,
            ..
        } => format!("Remove {wall_id} #{anchor_index}"),
        ProjectCommand::RemoveOpening { opening_id } => format!("Add Opening {opening_id}"),
        ProjectCommand::AddOpening(o) => format!("Remove Opening {}", o.id),
        ProjectCommand::RemoveFurniture { item_id } => format!("Place {item_id}"),
        ProjectCommand::PlaceFurniture(f) => format!("Remove {}", f.catalog_item_id),
        other => other.label(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PointType, Transform3D};

    #[test]
    fn undo_redo_roundtrip() {
        let mut project = Project::default();
        let mut history = CommandHistory::default();

        let level = Level {
            id: "lvl-1".into(),
            name: "EG".into(),
            elevation: 0.0,
            height: 2.5,
            floor_slab_thickness: 0.2,
            sort_order: 0,
        };
        history
            .execute(&mut project, ProjectCommand::AddLevel(level))
            .unwrap();

        let wall = Wall {
            id: "w-1".into(),
            level_id: "lvl-1".into(),
            is_closed_loop: false,
            thickness: 0.24,
            start_height: 2.5,
            end_height: 2.5,
            anchors: vec![WallAnchorPoint {
                id: "a-1".into(),
                pos_x: 0.0,
                pos_y: 0.0,
                handle_in_x: None,
                handle_in_y: None,
                handle_out_x: None,
                handle_out_y: None,
                point_type: PointType::Corner,
            }],
        };
        history
            .execute(&mut project, ProjectCommand::AddWall(wall))
            .unwrap();

        let moved = WallAnchorPoint {
            id: "a-1".into(),
            pos_x: 3.5,
            pos_y: 1.0,
            handle_in_x: None,
            handle_in_y: None,
            handle_out_x: None,
            handle_out_y: None,
            point_type: PointType::Corner,
        };
        history
            .execute(
                &mut project,
                ProjectCommand::UpdateWallAnchor {
                    wall_id: "w-1".into(),
                    anchor_index: 0,
                    anchor: moved,
                },
            )
            .unwrap();
        assert_eq!(project.walls[0].anchors[0].pos_x, 3.5);

        assert!(history.undo(&mut project).unwrap());
        assert_eq!(project.walls[0].anchors[0].pos_x, 0.0);

        assert!(history.redo(&mut project).unwrap());
        assert_eq!(project.walls[0].anchors[0].pos_x, 3.5);

        let item = FurnitureItem {
            id: "f-1".into(),
            level_id: "lvl-1".into(),
            catalog_item_id: "sofa".into(),
            asset_id: None,
            transform: Transform3D::default(),
            wall_mount_wall_id: None,
            ha_binding_json: None,
        };
        history
            .execute(&mut project, ProjectCommand::PlaceFurniture(item))
            .unwrap();
        assert_eq!(project.furniture.len(), 1);
        assert!(!history.can_redo());
    }
}
