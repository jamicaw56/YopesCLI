//! The JSON schemas for every tool, sent to the provider with each request.
//!
//! One entry per tool in `TOOL_NAMES`; the test in `mod.rs` checks the two
//! lists stay in sync.

use serde_json::{Value, json};

/// The `computer_use` schema. It lives in its own literal because the
/// serde_json macro has a fixed recursion budget, and this one item is as
/// deep as the rest of the tool list put together.
fn computer_use_schema() -> Value {
    json!(        {
        "type": "function",
        "function": {
            "name": "computer_use",
            "description": "Drive the desktop on Windows: point, click, scroll, drag, type text, press keys, wait between steps, focus a window and crop the capture to a region, then see the result. The agent has its own visible pointer: a click-through topmost arrow follows mouse steps on the desktop and remains between calls, while mouse steps post button messages to the window under the point so the system cursor never moves and the person at the keyboard keeps their mouse. The arrow is also drawn into the capture as a highlighted cursor, with its hotspot on the point the call last aimed at. One screenshot is captured before the actions and one after; the 'after' screenshot is attached to this tool result so a vision-capable model sees what changed. The result reports the frame origin and size in screen pixels, the desktop bounds and where the agent's pointer ended up: add the frame origin to a pixel you pick in the image to get the screen coordinate to click. Coordinates are screen pixels, so the screenshot tells you where to aim. Windows only.",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "What this call is trying to do, in one line. Shown to the user in the approval prompt and kept in the result."
                    },
                    "screen": {
                        "type": "integer",
                        "description": "Display to capture, 0-based, for multi-monitor desktops. Defaults to 0."
                    },
                    "region": {
                        "type": "object",
                        "description": "Capture only this rectangle of the screen, in screen pixels. A cropped frame is far easier to read than a whole 4K desktop, and the reported frame origin tells you how to map an image pixel back to a screen coordinate. Takes precedence over screen.",
                        "properties": {
                            "x": {
                                "type": "integer",
                                "description": "Left edge of the crop, in screen pixels."
                            },
                            "y": {
                                "type": "integer",
                                "description": "Top edge of the crop, in screen pixels."
                            },
                            "width": {
                                "type": "integer",
                                "description": "Crop width in pixels, above zero."
                            },
                            "height": {
                                "type": "integer",
                                "description": "Crop height in pixels, above zero."
                            }
                        },
                        "required": ["x", "y", "width", "height"]
                    },
                    "settle_ms": {
                        "type": "integer",
                        "description": "How long to let the desktop repaint after the actions before the second screenshot, in milliseconds. Defaults to 350, capped at 10000. Raise it for a slow app; use a wait step when the delay belongs between two clicks."
                    },
                    "actions": {
                        "type": "array",
                        "description": "Steps to run in order, at most 32. Leave empty to only take a screenshot and look.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "type": {
                                    "type": "string",
                                    "enum": [
                                        "move",
                                        "click",
                                        "double_click",
                                        "scroll",
                                        "drag",
                                        "type",
                                        "key",
                                        "wait",
                                        "focus_window"
                                    ],
                                    "description": "The step kind."
                                },
                                "x": {
                                    "type": "integer",
                                    "description": "Screen pixel x for move, click or scroll."
                                },
                                "y": {
                                    "type": "integer",
                                    "description": "Screen pixel y for move, click or scroll."
                                },
                                "button": {
                                    "type": "string",
                                    "enum": ["left", "right", "middle"],
                                    "description": "Mouse button for click or drag. Defaults to left."
                                },
                                "count": {
                                    "type": "integer",
                                    "description": "Clicks to send at one spot, 1 to 3. Defaults to 1."
                                },
                                "amount": {
                                    "type": "integer",
                                    "description": "Scroll notches: positive scrolls up, negative down, -10 to 10."
                                },
                                "from_x": {
                                    "type": "integer",
                                    "description": "Drag start x."
                                },
                                "from_y": {
                                    "type": "integer",
                                    "description": "Drag start y."
                                },
                                "to_x": {
                                    "type": "integer",
                                    "description": "Drag end x."
                                },
                                "to_y": {
                                    "type": "integer",
                                    "description": "Drag end y."
                                },
                                "text": {
                                    "type": "string",
                                    "description": "Text to type into whatever holds focus."
                                },
                                "keys": {
                                    "type": "array",
                                    "items": {
                                        "type": "string"
                                    },
                                    "description": "Named keys to press, such as ENTER, TAB, ESC or F5."
                                },
                                "chord": {
                                    "type": "string",
                                    "description": "A key chord such as ctrl+s, ctrl+shift+t or alt+tab."
                                },
                                "ms": {
                                    "type": "integer",
                                    "description": "Wait steps: how long to pause in milliseconds. Defaults to 500, capped at 10000."
                                },
                                "title": {
                                    "type": "string",
                                    "description": "Part of the window title to bring to the front (focus_window steps). Matching is case-insensitive on the main window title."
                                }
                            },
                            "required": ["type"]
                        }
                    }
                },
                "required": ["action"]
            }
        }
    })
}

/// The `apply_patch` schema, its own literal to spare the macro's recursion
/// budget, like `computer_use`.
fn apply_patch_schema() -> Value {
    json!(
        {
            "type": "function",
            "function": {
                "name": "apply_patch",
                "description": "Apply a multi-file patch: add, delete, update and rename files in one call. Format:\n*** Begin Patch\n*** Add File: path\n+line\n*** Update File: path\n*** Move to: new/path (optional)\n@@ optional line to seek first\n context line\n-removed line\n+added line\n*** Delete File: path\n*** End Patch\nKeep 1-3 context lines around each change. Every hunk is checked before anything is written, so a patch applies fully or not at all.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "patch": {
                            "type": "string",
                            "description": "The full patch text, from *** Begin Patch to *** End Patch."
                        }
                    },
                    "required": ["patch"]
                }
            }
        }
    )
}

pub fn definitions() -> Value {
    let mut value = json!([
        {
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List files and directories in the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory path relative to the working directory. Defaults to ."
                        }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a UTF-8 text file, optionally a range of lines. A directory path lists its entries.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to the working directory."
                        },
                        "start_line": {
                            "type": "integer",
                            "description": "First line to read, starting at 1."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of lines to return."
                        }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "multi_read",
                "description": "Read several UTF-8 text files in one call, in order. With all=true every entry in paths is a folder and every file directly inside it is read. Read-only.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "paths": {
                            "type": "array",
                            "description": "File paths (or folder paths when all is true), relative to the working directory.",
                            "items": {
                                "type": "string"
                            }
                        },
                        "all": {
                            "type": "boolean",
                            "description": "Read every file directly inside each folder in paths instead of the paths themselves."
                        }
                    },
                    "required": ["paths"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_image",
                "description": "Attach a PNG or JPEG image from disk to the conversation so a vision-capable model can see it. Read-only.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Image path relative to the working directory."
                        }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "glob",
                "description": "Find files and directories by glob pattern, relative to the working directory. Supports * (any text within one path segment), ? (one character), and ** (any number of segments). Read-only.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern such as src/**/*.rs."
                        },
                        "path": {
                            "type": "string",
                            "description": "Directory to search under, relative to the working directory. Defaults to ."
                        }
                    },
                    "required": ["pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "grep",
                "description": "Search file contents with a plain substring and return file:line:text matches. Binary files, dependency and cache directories are skipped. Read-only.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Text to search for."
                        },
                        "path": {
                            "type": "string",
                            "description": "Directory to search under, relative to the working directory. Defaults to ."
                        },
                        "include": {
                            "type": "string",
                            "description": "Glob for file types, such as *.rs or src/*/*.ts."
                        },
                        "ignore_case": {
                            "type": "boolean",
                            "description": "Match without case sensitivity. Defaults to false."
                        }
                    },
                    "required": ["pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Create or overwrite a text file. Parent directories are created.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to the working directory."
                        },
                        "content": {
                            "type": "string",
                            "description": "Full file content to write."
                        }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace exact text in an existing UTF-8 file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to the working directory."
                        },
                        "old_text": {
                            "type": "string",
                            "description": "Exact text to find."
                        },
                        "new_text": {
                            "type": "string",
                            "description": "Replacement text."
                        },
                        "replace_all": {
                            "type": "boolean",
                            "description": "Replace every match instead of requiring exactly one."
                        }
                    },
                    "required": ["path", "old_text", "new_text"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "multi_edit",
                "description": "Apply several exact-text edits in one call. Every edit follows the edit_file rules against one in-memory state, so a batch either applies fully or refuses without touching any file. Sequential edits to the same file see each other's results.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "edits": {
                            "type": "array",
                            "description": "The edits, in order.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "File path relative to the working directory."
                                    },
                                    "old_text": {
                                        "type": "string",
                                        "description": "Exact text to find."
                                    },
                                    "new_text": {
                                        "type": "string",
                                        "description": "Replacement text."
                                    },
                                    "replace_all": {
                                        "type": "boolean",
                                        "description": "Replace every match in that file instead of requiring exactly one."
                                    }
                                },
                                "required": ["path", "old_text", "new_text"]
                            }
                        }
                    },
                    "required": ["edits"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "move_to",
                "description": "Change the working directory all relative paths resolve against. The path must exist and be a directory; moving outside the workspace or the temp root asks the user first. Use it when a task lives in another folder, and move back when done.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory to move into, absolute or relative to the current working directory."
                        }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "run_command",
                "description": "Run a shell command in the workspace and return its output. Set risk to true for anything destructive or hard to undo: deleting or overwriting files, git resets or force pushes, installing or uninstalling software, stopping services, or anything reaching outside the working directory. Risky commands ask the user before they run. When the user requests running commands or actions, execute them directly with this tool rather than just describing the command.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command to run from the working directory."
                        },
                        "risk": {
                            "type": "boolean",
                            "description": "True when the command is destructive or could discard work, so the user is asked first."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "description": "Maximum run time in seconds. Defaults to 120."
                        }
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "bash_background",
                "description": "Start one bash shell command in the background and get a job id. Its output lands in a log; the app notifies this conversation automatically when the job finishes or times out, so never poll bash_output in a loop. Stop it with bash_kill when it has served its purpose. Set risk to true for anything destructive or hard to undo so the user is asked first.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Bash command to start."
                        },
                        "risk": {
                            "type": "boolean",
                            "description": "True when the command is destructive or could discard work, so the user is asked first."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "description": "Kill the job after this many seconds. Defaults to 120."
                        }
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "bash_output",
                "description": "Read a background bash job: everything it printed so far and whether it still runs or has exited. Pass the job id from bash_background. Usually not needed: the app notifies the conversation automatically when the job finishes, so call this only to peek at progress mid-run, never in a loop.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "job_id": {
                            "type": "integer",
                            "description": "The id bash_background returned."
                        }
                    },
                    "required": ["job_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "message_subagent",
                "description": "Send a correction to one running subagent: the message lands in its conversation as a user message and it continues, answering within its run. Pass the run id from a delegate call's result. Use it to fix course mid-run instead of waiting for a wrong answer.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "run": {
                            "type": "integer",
                            "description": "The subagent run id, shown in its tool result as [subagent run N]."
                        },
                        "message": {
                            "type": "string",
                            "description": "The correction or extra instruction."
                        }
                    },
                    "required": ["run", "message"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "stop_subagent",
                "description": "Ask one running subagent to stop: it answers with what it has instead of being killed mid-work. Pass the run id from a delegate call's result.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "run": {
                            "type": "integer",
                            "description": "The subagent run id, shown in its tool result as [subagent run N]."
                        }
                    },
                    "required": ["run"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "bash_kill",
                "description": "Stop a background bash job by its id. A finished or unknown job answers so, no need to kill twice.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "job_id": {
                            "type": "integer",
                            "description": "The id bash_background returned."
                        }
                    },
                    "required": ["job_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the web and return the top results with titles, URLs and snippets. Use it for current information, documentation, release notes, or anything the user asks about that is newer than your knowledge. Cite the URL you used.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query."
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Maximum number of results. Defaults to 5."
                        }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "web_fetch",
                "description": "Fetch one web page over HTTP GET and return its readable text: scripts, styles and tags are stripped. Use it when search results point at a page worth reading or when the user gives an address.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Absolute http:// or https:// address."
                        }
                    },
                    "required": ["url"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "todo_write",
                "description": "Maintain the session task checklist dynamically. Rewrite the whole list each call: every item is an object with the task text and a done flag. Send an empty items list to close the checklist once everything is done. The list is stored with the session, not as a file. You MUST use this tool dynamically to track any multi-step plan, updating task done states as you work.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "items": {
                            "type": "array",
                            "description": "The full checklist.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "text": {
                                        "type": "string",
                                        "description": "The task."
                                    },
                                    "done": {
                                        "type": "boolean",
                                        "description": "Whether the task is finished. Defaults to false."
                                    }
                                },
                                "required": ["text"]
                            }
                        }
                    },
                    "required": ["items"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "delegate",
                "description": "Hand one self-contained subtask to a subagent running on the same provider and model. The call returns at once with a run id and the subagent works in the background: keep working, do not wait. The subagent runs with the same tools you have (files, shell, web) in the same workspace, but it starts with an empty conversation, so the task must still carry every fact and instruction it needs. Several delegate calls in one turn run in parallel and stream live in the sidebar; the user can read every response with /agents. A subagent saying it is done does not finish the run: yopes checks the files it actually changed and the commands it ran, and reports a verdict (completed, failed, blocked or cancelled) with that evidence. So state the deliverable: pass expect_files for the files the task must change, and verify_command for the check that proves it works.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task": {
                            "type": "string",
                            "description": "Complete instructions for the subagent, including all context it needs."
                        },
                        "kind": {
                            "type": "string",
                            "enum": ["change", "read_only"],
                            "description": "What the task must produce. `change` (the default) must change at least one file; `read_only` must answer and should change nothing."
                        },
                        "expect_files": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Files the task must change, relative to the workspace. A run that skips one is reported failed instead of completed."
                        },
                        "verify_command": {
                            "type": "string",
                            "description": "A command yopes runs after the subagent submits, e.g. `cargo test`. A non-zero exit fails the run."
                        },
                        "timeout_minutes": {
                            "type": "integer",
                            "description": "How long the run may work before it is stopped and judged on what it has. Default 45, maximum 240."
                        }
                    },
                    "required": ["task"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "check_session",
                "description": "List the other open sessions (tabs) this conversation can talk to. Returns every tab's number, title, and whether it is mid-turn; the row marked `this session` is you. Address a tab by its number in talk_to. Read-only.",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "talk_to",
                "description": "Send a message to another open session (tab). The message is delivered into that conversation: when the tab is idle its model wakes and reads it at once; when it is busy the message waits and lands when its current turn ends. The tool result says which happened. Tab numbers come from check_session (1-based).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "tab": {
                            "type": "integer",
                            "description": "Tab number from check_session, 1-based."
                        },
                        "message": {
                            "type": "string",
                            "description": "The message text delivered into that session's conversation."
                        }
                    },
                    "required": ["tab", "message"]
                }
            }
        }
    ]);
    // One schema per literal: the macro expands each item itself, so adding
    // the deepest tool here would exhaust the recursion limit.
    if let Some(items) = value.as_array_mut() {
        items.push(apply_patch_schema());
        items.push(computer_use_schema());
    }
    value
}
