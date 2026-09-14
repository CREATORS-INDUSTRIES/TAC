import { useEffect, useRef, useState } from "react";

const API = "http://localhost:3001/position";
const KNOB = 40;
const FULL_TRAVEL_S = 2;

export default function App() {
  const [position, setPosition] = useState(50);
  const [transition, setTransition] = useState("none");
  const trackRef = useRef(null);
  const dragging = useRef(false);
  const shown = useRef(50);

  // Duration scales with distance: constant cruise speed like a motor.
  const moveTo = (target) => {
    const seconds = Math.max(0.15, (Math.abs(target - shown.current) / 100) * FULL_TRAVEL_S);
    setTransition(`left ${seconds}s cubic-bezier(0.4, 0, 0.2, 1)`);
    setPosition(target);
    shown.current = target;
  };

  useEffect(() => {
    const pull = () =>
      fetch(API)
        .then((r) => r.json())
        .then((d) => {
          if (!dragging.current) moveTo(d.position);
        })
        .catch(() => {});
    pull();
    const id = setInterval(pull, 1000);
    return () => clearInterval(id);
  }, []);

  const positionFromMouse = (clientX) => {
    const { left, width } = trackRef.current.getBoundingClientRect();
    return Math.min(100, Math.max(0, ((clientX - left) / width) * 100));
  };

  const onMouseDown = (e) => {
    e.preventDefault();
    dragging.current = true;
    setTransition("none");
    const move = (ev) => {
      shown.current = positionFromMouse(ev.clientX);
      setPosition(shown.current);
    };
    const up = () => {
      dragging.current = false;
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  return (
    <div style={{ display: "grid", placeItems: "center", height: "100vh", fontFamily: "sans-serif" }}>
      <div>
        <div
          ref={trackRef}
          style={{ position: "relative", width: 600, height: 4, background: "#999" }}
        >
          <div
            onMouseDown={onMouseDown}
            style={{
              position: "absolute",
              top: "50%",
              left: `${position}%`,
              transform: "translate(-50%, -50%)",
              transition,
              width: KNOB,
              height: KNOB,
              background: "#e33",
              cursor: "grab",
              userSelect: "none",
            }}
          />
        </div>
        <p style={{ textAlign: "center" }}>{position.toFixed(1)}%</p>
      </div>
    </div>
  );
}
