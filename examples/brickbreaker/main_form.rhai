// Brick Breaker: an Arkanoid-style game drawn on a Canvas.
//
// Controls: Left/Right (or A/D) or the mouse move the paddle, Space or a
// click launches the ball, P pauses.
//
// The game's state is plain top-level variables. Every function and handler
// below reads and assigns them directly (`score += 10`), so there is nothing
// to pass around or store back.
//
// The game's modes: "title" (before the first serve), "ready" (the ball sits
// on the paddle), "playing", "paused", "over" and "won".

// ---- State -----------------------------------------------------------------

const PADDLE_W = 96.0;
const PADDLE_H = 14.0;
const BALL_R = 7.0;
const MAX_SPEED = 650.0;

// The canvas size is only known once the form is built, so form_load sets
// width, height and the paddle's row through new_game().
let width = 0.0;
let height = 0.0;
let mode = "title";
let score = 0;
let lives = 3;
let paddle_x = 0.0;
let paddle_y = 0.0;
let ball_x = 0.0;
let ball_y = 0.0;
let ball_vx = 0.0;
let ball_vy = 0.0;
let speed = 300.0;               // pixels per second; it creeps up during play
let bricks = [];
let bricks_left = 0;

// ---- Events ----------------------------------------------------------------

fn form_load() {
    new_game();
    game_canvas.focus();          // so the arrow keys reach the canvas
}

fn game_canvas_frame(dt) {
    update(dt);
    draw();
}

fn game_canvas_key_down(key) {
    if key == "space" {
        start_or_launch();
    } else if key == "p" {
        toggle_pause();
    }
}

fn game_canvas_mouse_move(x, y) {
    if mode != "paused" {
        paddle_x = clamp(x - PADDLE_W / 2.0, 0.0, width - PADDLE_W);
    }
}

fn game_canvas_mouse_down(x, y, button) {
    if button == "left" {
        start_or_launch();
    }
}

// ---- Setting up ------------------------------------------------------------

// A fresh game: three lives, a full wall of bricks, the ball on the paddle.
fn new_game() {
    width = game_canvas.width.to_float();
    height = game_canvas.height.to_float();
    mode = "title";
    score = 0;
    lives = 3;
    speed = 300.0;
    paddle_x = (width - PADDLE_W) / 2.0;
    paddle_y = height - 40.0;
    bricks = make_bricks();
    bricks_left = bricks.len();
    stick_ball();
}

// Six rainbow rows of ten bricks; the top two rows take two hits.
fn make_bricks() {
    let colors = [[231, 76, 60], [230, 126, 34], [241, 196, 15],
                  [46, 204, 113], [52, 152, 219], [155, 89, 182]];
    let columns = 10;
    let margin = 16.0;
    let gap = 4.0;
    let w = (width - 2.0 * margin - gap * (columns - 1)) / columns;
    let wall = [];
    for row in 0..colors.len() {
        for column in 0..columns {
            wall.push(#{
                x: margin + column * (w + gap),
                y: 64.0 + row * 24.0,
                w: w,
                h: 20.0,
                hits: if row < 2 { 2 } else { 1 },
                points: if row < 2 { 20 } else { 10 },
                rgb: colors[row],
            });
        }
    }
    wall
}

// ---- Rules -----------------------------------------------------------------

// Space or a click: start a new game after the last one ended, then serve.
fn start_or_launch() {
    if mode == "over" || mode == "won" {
        new_game();
    }
    if mode == "title" || mode == "ready" {
        // Serve straight up, give or take a few degrees.
        let angle = (random() - 0.5) * 0.6;
        ball_vx = speed * sin(angle);
        ball_vy = -speed * cos(angle);
        mode = "playing";
    }
}

fn toggle_pause() {
    if mode == "playing" {
        mode = "paused";
    } else if mode == "paused" {
        mode = "playing";
    }
}

// Puts the ball on top of the paddle's middle.
fn stick_ball() {
    ball_x = paddle_x + PADDLE_W / 2.0;
    ball_y = paddle_y - BALL_R - 1.0;
}

// Advances the game by `dt` seconds.
fn update(dt) {
    if mode != "title" && mode != "ready" && mode != "playing" {
        return;
    }
    move_paddle(dt);
    if mode != "playing" {
        stick_ball();
        return;
    }
    speed = min(speed + 4.0 * dt, MAX_SPEED);
    // Small steps, so a fast ball cannot skip through a 20-pixel brick.
    let steps = max(1, (speed * dt / 4.0).ceiling().to_int());
    for step in 0..steps {
        move_ball(dt / steps);
        if mode != "playing" {
            break;
        }
    }
}

fn move_paddle(dt) {
    let direction = 0.0;
    if game_canvas.is_key_down("left") || game_canvas.is_key_down("a") { direction -= 1.0; }
    if game_canvas.is_key_down("right") || game_canvas.is_key_down("d") { direction += 1.0; }
    let x = paddle_x + direction * 480.0 * dt;
    paddle_x = clamp(x, 0.0, width - PADDLE_W);
}

fn move_ball(dt) {
    let r = BALL_R;
    ball_x += ball_vx * dt;
    ball_y += ball_vy * dt;

    // The side walls and the bottom of the score bar.
    if ball_x < r { ball_x = r; ball_vx = abs(ball_vx); }
    if ball_x > width - r { ball_x = width - r; ball_vx = -abs(ball_vx); }
    if ball_y < 32.0 + r { ball_y = 32.0 + r; ball_vy = abs(ball_vy); }

    // The paddle: the further from its middle the ball lands, the wider the
    // angle it leaves at (up to about 60 degrees).
    if ball_vy > 0.0 && rects_overlap(ball_x - r, ball_y - r, 2.0 * r, 2.0 * r,
                                      paddle_x, paddle_y, PADDLE_W, PADDLE_H) {
        let middle = paddle_x + PADDLE_W / 2.0;
        let offset = clamp((ball_x - middle) / (PADDLE_W / 2.0), -1.0, 1.0);
        ball_vx = speed * sin(offset * 1.05);
        ball_vy = -speed * cos(offset * 1.05);
        ball_y = paddle_y - r;
    }

    hit_brick();

    // Missed: lose a life, or the game.
    if ball_y - r > height {
        lives -= 1;
        if lives > 0 {
            mode = "ready";
            stick_ball();
        } else {
            mode = "over";
        }
    }
}

// Bounces the ball off the first brick it touches and damages that brick.
fn hit_brick() {
    let r = BALL_R;
    let left = ball_x - r;
    let top = ball_y - r;
    let size = 2.0 * r;
    for i in 0..bricks.len() {
        let b = bricks[i];
        if b.hits > 0 && rects_overlap(left, top, size, size, b.x, b.y, b.w, b.h) {
            // Bounce along the axis the ball went in least.
            let push_x = min(left + size - b.x, b.x + b.w - left);
            let push_y = min(top + size - b.y, b.y + b.h - top);
            // Bounce, and move the ball just outside the brick on that side,
            // so the next step cannot hit the same brick again.
            if push_x < push_y {
                let from_left = ball_x < b.x + b.w / 2.0;
                ball_vx = if from_left { -abs(ball_vx) } else { abs(ball_vx) };
                ball_x = if from_left { b.x - r } else { b.x + b.w + r };
            } else {
                let from_above = ball_y < b.y + b.h / 2.0;
                ball_vy = if from_above { -abs(ball_vy) } else { abs(ball_vy) };
                ball_y = if from_above { b.y - r } else { b.y + b.h + r };
            }
            bricks[i].hits = b.hits - 1;
            if b.hits == 1 {
                score += b.points;
                bricks_left -= 1;
            } else {
                score += 5;
            }
            speed = min(speed + 6.0, MAX_SPEED);
            keep_speed();
            if bricks_left == 0 {
                mode = "won";
            }
            return;
        }
    }
}

// Rescales the ball's velocity to the current speed, keeping its direction.
fn keep_speed() {
    let length = sqrt(ball_vx * ball_vx + ball_vy * ball_vy);
    if length > 0.0 {
        ball_vx = ball_vx / length * speed;
        ball_vy = ball_vy / length * speed;
    }
}

// ---- Drawing ---------------------------------------------------------------

fn draw() {
    game_canvas.clear("#101018");
    draw_bricks();
    game_canvas.fill_round_rect(paddle_x, paddle_y, PADDLE_W, PADDLE_H, 7, "#ecf0f1");
    game_canvas.fill_circle(ball_x, ball_y, BALL_R, "#ffe066");
    draw_score_bar();
    draw_message();
}

fn draw_bricks() {
    for b in bricks {
        if b.hits > 0 {
            // A brick that still takes two hits is drawn darker.
            let k = if b.hits > 1 { 0.55 } else { 1.0 };
            let red = b.rgb[0] * k;
            let green = b.rgb[1] * k;
            let blue = b.rgb[2] * k;
            game_canvas.fill_round_rect(b.x, b.y, b.w, b.h, 3, rgb(red, green, blue));
            game_canvas.fill_rect(b.x + 3, b.y + 2, b.w - 6, 3,
                                  rgb(red + 40, green + 40, blue + 40));
        }
    }
}

fn draw_score_bar() {
    game_canvas.fill_rect(0, 0, width, 32, "#1c1c2c");
    game_canvas.text(12, 7, `Score: ${score}`, "#ffffff", 16);
    for i in 0..lives {
        game_canvas.fill_circle(width - 20 - i * 18, 16, 6, "#e74c3c");
    }
}

// The banner for the modes that wait for the player.
fn draw_message() {
    let y = height / 2.0 + 20.0;
    if mode == "title" {
        centered("BRICK BREAKER", y - 30.0, 32, "#f5b041");
        centered("Press Space or click to start", y + 16.0, 16, "#ffffff");
        centered("Left/Right or the mouse to move · P to pause", y + 40.0, 13, "#9aa0b4");
    } else if mode == "ready" {
        centered("Press Space to launch", y, 16, "#ffffff");
    } else if mode == "paused" {
        centered("Paused — press P", y, 20, "#ffffff");
    } else if mode == "over" {
        centered("Game over — press Space", y, 24, "#e74c3c");
        centered(`Final score: ${score}`, y + 34.0, 16, "#ffffff");
    } else if mode == "won" {
        centered("You win!", y, 32, "#2ecc71");
        centered(`Score: ${score} — press Space to play again`, y + 44.0, 16, "#ffffff");
    }
}

// Draws `message` centred across the canvas, `y` pixels from the top.
fn centered(message, y, size, color) {
    let w = game_canvas.text_width(message, size);
    game_canvas.text((game_canvas.width - w) / 2.0, y, message, color, size);
}
