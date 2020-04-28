use crate::err::Result;
use crate::graphics::RenderChain;
use crate::model::Graph;
use crate::video_node::{
    EffectNode, MediaNode, OutputNode, VideoNode, VideoNodeId, VideoNodeKind, VideoNodeKindMut,
};

use log::*;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, HtmlElement, WebGl2RenderingContext};

#[wasm_bindgen]
pub struct Model {
    canvas_el: HtmlCanvasElement,
    graph: Graph,
    chain: RenderChain,
}

#[wasm_bindgen]
impl Model {
    #[wasm_bindgen(constructor)]
    pub fn new(canvas: JsValue, size: i32) -> Option<Model> {
        let canvas_el = canvas.dyn_into::<HtmlCanvasElement>().ok()?;
        let context = Rc::new(
            canvas_el
                .get_context("webgl2")
                .expect("WebGL2 not supported")
                .unwrap()
                .dyn_into::<WebGl2RenderingContext>()
                .unwrap(),
        );
        let chain_size = (size, size);
        let chain = RenderChain::new(context, chain_size).unwrap();
        Some(Model {
            graph: Graph::new(),
            canvas_el,
            chain,
        })
    }

    /// This is a temporary utility function that will get refactored
    #[wasm_bindgen]
    pub fn append_node(&mut self, name: &str, value: f64) -> JsValue {
        let mut node = EffectNode::new(name).ok().unwrap();
        node.set_intensity(value);

        let id = node.id();
        self.graph.add_videonode(node);

        JsValue::from_serde(&serde_json::to_value(id).unwrap()).unwrap()
    }

    #[wasm_bindgen]
    pub fn render(&mut self, time: f64) -> std::result::Result<(), JsValue> {
        self.render_internal(time).map_err(|e| e.into())
    }

    fn render_internal(&mut self, time: f64) -> Result<()> {
        // Resize the canvas to if it changed size
        // This doesn't need to happen before rendering; just before calling `paint_node()`
        let canvas_width = self.canvas_el.client_width() as u32;
        let canvas_height = self.canvas_el.client_height() as u32;
        if self.canvas_el.width() != canvas_width {
            self.canvas_el.set_width(canvas_width);
        }
        if self.canvas_el.height() != canvas_height {
            self.canvas_el.set_height(canvas_height);
        }

        // Perform pre-render operations that may modify each node
        self.chain.pre_render(self.graph.nodes_mut(), time);

        // Render each node sequentially in topological order
        self.chain
            .context
            .viewport(0, 0, self.chain.size.0, self.chain.size.1);
        for node in self.graph.toposort() {
            let fbos = self
                .graph
                .node_inputs(node)
                .iter()
                .map(|n| n.and_then(|node| self.chain.node_fbo(node)))
                .collect::<Vec<_>>();
            self.chain.render_node(node, &fbos);
        }
        Ok(())
    }

    #[wasm_bindgen]
    pub fn paint_node(
        &mut self,
        id: JsValue,
        node_el: JsValue,
    ) -> std::result::Result<(), JsValue> {
        let node_ref = node_el.dyn_into::<HtmlElement>()?;
        let id: VideoNodeId = id
            .into_serde()
            .map_err(|_| JsValue::from_str("Invalid id, expected Number"))?;
        self.paint_node_internal(id, node_ref).map_err(|e| e.into())
    }

    fn paint_node_internal(&mut self, id: VideoNodeId, node_ref: HtmlElement) -> Result<()> {
        // This assumes that the canvas has style: "position: fixed; left: 0; right: 0;"
        let node = self.graph.node(id).ok_or("Invalid node id")?;

        let canvas_size = (
            self.chain.context.drawing_buffer_width(),
            self.chain.context.drawing_buffer_height(),
        );
        let node_rect = node_ref.get_bounding_client_rect();
        let canvas_rect = self.canvas_el.get_bounding_client_rect();

        let x_ratio = canvas_rect.width() / canvas_size.0 as f64;
        let y_ratio = canvas_rect.height() / canvas_size.1 as f64;
        let left = ((node_rect.left() - canvas_rect.left()) / x_ratio).ceil();
        let right = ((node_rect.right() - canvas_rect.left()) / x_ratio).floor();
        let top = ((node_rect.top() - canvas_rect.top()) / y_ratio).ceil();
        let bottom = ((node_rect.bottom() - canvas_rect.top()) / y_ratio).floor();

        self.chain.context.viewport(
            left as i32,
            canvas_size.1 - bottom as i32,
            (right - left) as i32,
            (bottom - top) as i32,
        );
        self.chain.paint(node).unwrap();
        Ok(())
    }

    #[wasm_bindgen]
    pub fn state(&self) -> JsValue {
        JsValue::from_serde(&self.graph.state()).unwrap()
    }

    #[wasm_bindgen]
    pub fn set_state(&mut self, state: JsValue) {
        info!("raw: {:?}", state);
        let v: serde_json::Value = state.into_serde().unwrap();
        self.graph.set_state(v).unwrap();
    }
}
