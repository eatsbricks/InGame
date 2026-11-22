use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtectionState {
    Standby,
    Protected,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionEvent {
    ForegroundChanged {
        hwnd: isize,
        is_pubg: bool,
    },
    WindowMinimized {
        hwnd: isize,
    },
    EscapeTriggered,
    SetEnabled(bool),
    WatchdogTick {
        current_fg: isize,
        pubg_window_alive: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionAction {
    Arm { pubg_hwnd: isize },
    Disarm,
    ExecuteEscapeExit { pubg_hwnd: isize },
}

#[derive(Debug, Clone)]
pub struct StateMachine {
    enabled: bool,
    state: ProtectionState,
    active_pubg_hwnd: Option<isize>,
    exiting_hwnd: Option<isize>,
}

impl StateMachine {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            state: if enabled {
                ProtectionState::Standby
            } else {
                ProtectionState::Paused
            },
            active_pubg_hwnd: None,
            exiting_hwnd: None,
        }
    }

    pub fn state(&self) -> ProtectionState {
        self.state
    }

    pub fn active_pubg_hwnd(&self) -> Option<isize> {
        self.active_pubg_hwnd
    }

    pub fn exiting_hwnd(&self) -> Option<isize> {
        self.exiting_hwnd
    }

    pub fn handle(&mut self, event: ProtectionEvent) -> Vec<ProtectionAction> {
        match event {
            ProtectionEvent::ForegroundChanged { hwnd, is_pubg } => {
                self.handle_foreground_changed(hwnd, is_pubg)
            }
            ProtectionEvent::WindowMinimized { hwnd } => self.handle_window_minimized(hwnd),
            ProtectionEvent::EscapeTriggered => self.handle_escape_triggered(),
            ProtectionEvent::SetEnabled(enabled) => self.handle_set_enabled(enabled),
            ProtectionEvent::WatchdogTick {
                current_fg,
                pubg_window_alive,
            } => self.handle_watchdog_tick(current_fg, pubg_window_alive),
        }
    }

    fn handle_foreground_changed(&mut self, hwnd: isize, is_pubg: bool) -> Vec<ProtectionAction> {
        if let Some(exiting) = self.exiting_hwnd {
            if exiting == hwnd && is_pubg {
                return Vec::new();
            }
            self.exiting_hwnd = None;
        }

        if !is_pubg {
            if self.state == ProtectionState::Protected {
                self.state = ProtectionState::Standby;
                self.active_pubg_hwnd = None;
                return vec![ProtectionAction::Disarm];
            }

            return Vec::new();
        }

        if !self.enabled {
            return Vec::new();
        }

        match self.state {
            ProtectionState::Standby => {
                self.state = ProtectionState::Protected;
                self.active_pubg_hwnd = Some(hwnd);
                vec![ProtectionAction::Arm { pubg_hwnd: hwnd }]
            }
            ProtectionState::Protected => {
                if self.active_pubg_hwnd == Some(hwnd) {
                    Vec::new()
                } else {
                    self.active_pubg_hwnd = Some(hwnd);
                    vec![
                        ProtectionAction::Disarm,
                        ProtectionAction::Arm { pubg_hwnd: hwnd },
                    ]
                }
            }
            ProtectionState::Paused => Vec::new(),
        }
    }

    fn handle_window_minimized(&mut self, hwnd: isize) -> Vec<ProtectionAction> {
        if self.exiting_hwnd == Some(hwnd) {
            self.exiting_hwnd = None;
        }

        if self.state == ProtectionState::Protected && self.active_pubg_hwnd == Some(hwnd) {
            self.state = ProtectionState::Standby;
            self.active_pubg_hwnd = None;
            return vec![ProtectionAction::Disarm];
        }

        Vec::new()
    }

    fn handle_escape_triggered(&mut self) -> Vec<ProtectionAction> {
        if self.state != ProtectionState::Protected {
            return Vec::new();
        }

        if let Some(pubg_hwnd) = self.active_pubg_hwnd.take() {
            self.state = ProtectionState::Standby;
            self.exiting_hwnd = Some(pubg_hwnd);
            vec![ProtectionAction::ExecuteEscapeExit { pubg_hwnd }]
        } else {
            self.state = ProtectionState::Standby;
            Vec::new()
        }
    }

    fn handle_set_enabled(&mut self, enabled: bool) -> Vec<ProtectionAction> {
        self.enabled = enabled;

        if !enabled {
            self.exiting_hwnd = None;
            let was_protected = self.state == ProtectionState::Protected;
            self.state = ProtectionState::Paused;
            self.active_pubg_hwnd = None;
            if was_protected {
                return vec![ProtectionAction::Disarm];
            }
            return Vec::new();
        }

        if self.state == ProtectionState::Paused {
            self.state = ProtectionState::Standby;
            self.active_pubg_hwnd = None;
        }

        Vec::new()
    }

    fn handle_watchdog_tick(
        &mut self,
        current_fg: isize,
        pubg_window_alive: bool,
    ) -> Vec<ProtectionAction> {
        if let Some(exiting) = self.exiting_hwnd {
            if current_fg != exiting || !pubg_window_alive {
                self.exiting_hwnd = None;
            }
        }

        if self.state == ProtectionState::Protected {
            let still_valid = match self.active_pubg_hwnd {
                Some(pubg_hwnd) => pubg_window_alive && current_fg == pubg_hwnd,
                None => false,
            };

            if !still_valid {
                self.state = ProtectionState::Standby;
                self.active_pubg_hwnd = None;
                return vec![ProtectionAction::Disarm];
            }
        }

        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standby_arms_on_pubg_foreground_and_disarms_on_other_window() {
        let mut sm = StateMachine::new(true);
        assert_eq!(sm.state(), ProtectionState::Standby);
        assert_eq!(sm.active_pubg_hwnd(), None);

        let actions = sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });
        assert_eq!(sm.state(), ProtectionState::Protected);
        assert_eq!(sm.active_pubg_hwnd(), Some(100));
        assert_eq!(actions, vec![ProtectionAction::Arm { pubg_hwnd: 100 }]);

        // Repeated foreground event for same PUBG window is a no-op
        let dup_actions = sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });
        assert_eq!(sm.state(), ProtectionState::Protected);
        assert!(dup_actions.is_empty());

        let actions = sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 200,
            is_pubg: false,
        });
        assert_eq!(sm.state(), ProtectionState::Standby);
        assert_eq!(sm.active_pubg_hwnd(), None);
        assert_eq!(actions, vec![ProtectionAction::Disarm]);
    }

    #[test]
    fn escape_exit_suppresses_rearm_until_focus_leaves_pubg() {
        let mut sm = StateMachine::new(true);
        assert_eq!(
            sm.handle(ProtectionEvent::ForegroundChanged {
                hwnd: 100,
                is_pubg: true,
            }),
            vec![ProtectionAction::Arm { pubg_hwnd: 100 }]
        );

        let actions = sm.handle(ProtectionEvent::EscapeTriggered);
        assert_eq!(sm.state(), ProtectionState::Standby);
        assert_eq!(sm.active_pubg_hwnd(), None);
        assert_eq!(
            actions,
            vec![ProtectionAction::ExecuteEscapeExit { pubg_hwnd: 100 }]
        );

        // Flicker while PUBG is still foreground must NOT re-arm
        assert!(sm
            .handle(ProtectionEvent::ForegroundChanged {
                hwnd: 100,
                is_pubg: true,
            })
            .is_empty());
        assert_eq!(sm.state(), ProtectionState::Standby);

        assert!(sm
            .handle(ProtectionEvent::WatchdogTick {
                current_fg: 100,
                pubg_window_alive: true,
            })
            .is_empty());
        assert_eq!(sm.state(), ProtectionState::Standby);

        // Once focus leaves PUBG, suppression clears
        assert!(sm
            .handle(ProtectionEvent::ForegroundChanged {
                hwnd: 200,
                is_pubg: false,
            })
            .is_empty());
        assert_eq!(sm.state(), ProtectionState::Standby);

        // Returning to PUBG now re-arms cleanly
        let actions = sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });
        assert_eq!(sm.state(), ProtectionState::Protected);
        assert_eq!(sm.active_pubg_hwnd(), Some(100));
        assert_eq!(actions, vec![ProtectionAction::Arm { pubg_hwnd: 100 }]);
    }

    #[test]
    fn escape_suppression_also_clears_via_watchdog_when_focus_moves_or_window_dies() {
        let mut sm = StateMachine::new(true);
        sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });
        sm.handle(ProtectionEvent::EscapeTriggered);

        // Watchdog observes foreground left hwnd 100
        assert!(sm
            .handle(ProtectionEvent::WatchdogTick {
                current_fg: 200,
                pubg_window_alive: true,
            })
            .is_empty());

        // Subsequent PUBG focus can now arm again
        let actions = sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });
        assert_eq!(sm.state(), ProtectionState::Protected);
        assert_eq!(actions, vec![ProtectionAction::Arm { pubg_hwnd: 100 }]);

        // And when PUBG dies during exit suppression, suppression clears too
        sm.handle(ProtectionEvent::EscapeTriggered);
        assert!(sm
            .handle(ProtectionEvent::WatchdogTick {
                current_fg: 100,
                pubg_window_alive: false,
            })
            .is_empty());
        let actions = sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });
        assert_eq!(sm.state(), ProtectionState::Protected);
        assert_eq!(actions, vec![ProtectionAction::Arm { pubg_hwnd: 100 }]);
    }

    #[test]
    fn pause_disarms_immediately_and_resume_does_not_arm_until_pubg_foreground() {
        let mut sm = StateMachine::new(true);
        sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });
        assert_eq!(sm.state(), ProtectionState::Protected);

        // Pausing while Protected immediately disarms
        let actions = sm.handle(ProtectionEvent::SetEnabled(false));
        assert_eq!(sm.state(), ProtectionState::Paused);
        assert_eq!(sm.active_pubg_hwnd(), None);
        assert_eq!(actions, vec![ProtectionAction::Disarm]);

        // PUBG foreground while Paused does nothing
        let actions = sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });
        assert_eq!(sm.state(), ProtectionState::Paused);
        assert!(actions.is_empty());

        // Resuming returns to Standby without immediately arming
        let actions = sm.handle(ProtectionEvent::SetEnabled(true));
        assert_eq!(sm.state(), ProtectionState::Standby);
        assert_eq!(sm.active_pubg_hwnd(), None);
        assert!(actions.is_empty());

        // Subsequent PUBG foreground now arms
        let actions = sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });
        assert_eq!(sm.state(), ProtectionState::Protected);
        assert_eq!(actions, vec![ProtectionAction::Arm { pubg_hwnd: 100 }]);

        // Constructing disabled starts Paused
        let sm_paused = StateMachine::new(false);
        assert_eq!(sm_paused.state(), ProtectionState::Paused);
        assert_eq!(sm_paused.active_pubg_hwnd(), None);
    }

    #[test]
    fn watchdog_disarms_if_pubg_dies_or_loses_focus_silently() {
        // Case 1: PUBG window dies
        let mut sm = StateMachine::new(true);
        sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });

        // Healthy tick keeps Protected
        assert!(sm
            .handle(ProtectionEvent::WatchdogTick {
                current_fg: 100,
                pubg_window_alive: true,
            })
            .is_empty());
        assert_eq!(sm.state(), ProtectionState::Protected);

        let actions = sm.handle(ProtectionEvent::WatchdogTick {
            current_fg: 100,
            pubg_window_alive: false,
        });
        assert_eq!(sm.state(), ProtectionState::Standby);
        assert_eq!(sm.active_pubg_hwnd(), None);
        assert_eq!(actions, vec![ProtectionAction::Disarm]);

        // Case 2: PUBG silently loses focus to another window
        sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });
        let actions = sm.handle(ProtectionEvent::WatchdogTick {
            current_fg: 999,
            pubg_window_alive: true,
        });
        assert_eq!(sm.state(), ProtectionState::Standby);
        assert_eq!(sm.active_pubg_hwnd(), None);
        assert_eq!(actions, vec![ProtectionAction::Disarm]);
    }

    #[test]
    fn switching_between_different_pubg_hwnds_rearms_cleanly() {
        let mut sm = StateMachine::new(true);
        assert_eq!(
            sm.handle(ProtectionEvent::ForegroundChanged {
                hwnd: 100,
                is_pubg: true,
            }),
            vec![ProtectionAction::Arm { pubg_hwnd: 100 }]
        );

        assert_eq!(
            sm.handle(ProtectionEvent::ForegroundChanged {
                hwnd: 101,
                is_pubg: true,
            }),
            vec![
                ProtectionAction::Disarm,
                ProtectionAction::Arm { pubg_hwnd: 101 }
            ]
        );
        assert_eq!(sm.state(), ProtectionState::Protected);
        assert_eq!(sm.active_pubg_hwnd(), Some(101));
    }

    #[test]
    fn escape_triggered_outside_protected_is_ignored() {
        let mut sm = StateMachine::new(true);
        assert!(sm.handle(ProtectionEvent::EscapeTriggered).is_empty());
        assert_eq!(sm.state(), ProtectionState::Standby);

        sm.handle(ProtectionEvent::SetEnabled(false));
        assert!(sm.handle(ProtectionEvent::EscapeTriggered).is_empty());
        assert_eq!(sm.state(), ProtectionState::Paused);
    }

    #[test]
    fn window_minimized_clears_escape_exit_suppression_so_direct_return_to_pubg_rearms() {
        let mut sm = StateMachine::new(true);
        assert_eq!(
            sm.handle(ProtectionEvent::ForegroundChanged {
                hwnd: 100,
                is_pubg: true,
            }),
            vec![ProtectionAction::Arm { pubg_hwnd: 100 }]
        );

        // User presses F10 -> minimizes PUBG directly to desktop (Windows fires MINIMIZESTART,
        // NOT EVENT_SYSTEM_FOREGROUND for any other window).
        assert_eq!(
            sm.handle(ProtectionEvent::EscapeTriggered),
            vec![ProtectionAction::ExecuteEscapeExit { pubg_hwnd: 100 }]
        );
        assert_eq!(sm.exiting_hwnd(), Some(100));

        // Minimize event for PUBG clears exit suppression immediately.
        assert!(sm
            .handle(ProtectionEvent::WindowMinimized { hwnd: 100 })
            .is_empty());
        assert_eq!(sm.exiting_hwnd(), None);

        // Returning directly to PUBG must re-arm on the very first foreground event
        // (without requiring the user to press Win twice).
        let actions = sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });
        assert_eq!(sm.state(), ProtectionState::Protected);
        assert_eq!(sm.active_pubg_hwnd(), Some(100));
        assert_eq!(actions, vec![ProtectionAction::Arm { pubg_hwnd: 100 }]);
    }

    #[test]
    fn unrelated_window_minimized_does_not_disarm_pubg_or_clear_exiting() {
        let mut sm = StateMachine::new(true);
        sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });

        // Minimizing an unrelated window on another monitor must not disarm PUBG.
        assert!(sm
            .handle(ProtectionEvent::WindowMinimized { hwnd: 200 })
            .is_empty());
        assert_eq!(sm.state(), ProtectionState::Protected);
        assert_eq!(sm.active_pubg_hwnd(), Some(100));

        // Minimizing PUBG itself without F10 disarms immediately.
        assert_eq!(
            sm.handle(ProtectionEvent::WindowMinimized { hwnd: 100 }),
            vec![ProtectionAction::Disarm]
        );
        assert_eq!(sm.state(), ProtectionState::Standby);
        assert_eq!(sm.active_pubg_hwnd(), None);

        // During exit suppression for PUBG (100), minimizing an unrelated window (200) must not clear it.
        sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });
        sm.handle(ProtectionEvent::EscapeTriggered);
        assert!(sm
            .handle(ProtectionEvent::WindowMinimized { hwnd: 200 })
            .is_empty());
        assert_eq!(sm.exiting_hwnd(), Some(100));
    }

    #[test]
    fn non_pubg_foreground_for_exiting_hwnd_clears_suppression() {
        let mut sm = StateMachine::new(true);
        sm.handle(ProtectionEvent::ForegroundChanged {
            hwnd: 100,
            is_pubg: true,
        });
        sm.handle(ProtectionEvent::EscapeTriggered);

        // If classify_hwnd reports `is_pubg: false` for the exiting HWND (e.g. it became iconic),
        // suppression must clear so returning once visible re-arms immediately.
        assert!(sm
            .handle(ProtectionEvent::ForegroundChanged {
                hwnd: 100,
                is_pubg: false,
            })
            .is_empty());
        assert_eq!(sm.exiting_hwnd(), None);

        assert_eq!(
            sm.handle(ProtectionEvent::ForegroundChanged {
                hwnd: 100,
                is_pubg: true,
            }),
            vec![ProtectionAction::Arm { pubg_hwnd: 100 }]
        );
    }

    #[test]
    fn protection_state_serde_uses_pascal_case_variants() {
        assert_eq!(
            serde_json::to_string(&ProtectionState::Standby).unwrap(),
            "\"Standby\""
        );
        assert_eq!(
            serde_json::to_string(&ProtectionState::Protected).unwrap(),
            "\"Protected\""
        );
        assert_eq!(
            serde_json::to_string(&ProtectionState::Paused).unwrap(),
            "\"Paused\""
        );
    }
}
