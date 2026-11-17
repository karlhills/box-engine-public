-- A starting bonus, through the `score` API that Rust adds
-- (src/script_api.rs), and a registered Rust event sent by name.
local Bonus = {}

Bonus.properties = {
    points = { type = "integer", default = 5, min = 0, max = 100, doc = "Bonus points at the start" },
}

function Bonus.on_start(self)
    score.add(self.props.points)
    -- `PickedUp` is registered with `register_event`: the Rust handler adds it.
    events.send("PickedUp", { points = 1 })
    log.info("bonus given; score is now", score.get())
end

return Bonus
