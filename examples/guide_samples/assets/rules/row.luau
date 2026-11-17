local Row = {}
Row.properties = { count = { type = "integer", default = 3, min = 1, max = 20 } }

function Row.rule(intents: { RuleIntent }, out: RuleOut)
    for _, it in intents do
        for i = 1, it.props.count do
            out:part(it, `post{i}`, { Primitive = {} }, { position = vector.create(i * 2, 0.5, 0) })
        end
    end
end

return Row
