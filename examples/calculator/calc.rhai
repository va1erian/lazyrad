fn apply(left, operator, right) {
    if operator == "+" {
        left + right
    } else if operator == "-" {
        left - right
    } else if operator == "*" {
        left * right
    } else if operator == "/" {
        if right == 0.0 {
            0.0
        } else {
            left / right
        }
    } else {
        right
    }
}
