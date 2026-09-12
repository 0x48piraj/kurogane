import "./style.css";
import content from "../public/demo/content.txt?raw";

document.querySelector<HTMLDivElement>("#app")!.innerHTML = `
  <div>
    <p>Hello World<p/>
    <p>${content}<p/>
  </div>
`;
