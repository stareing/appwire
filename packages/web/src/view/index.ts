/** 界面级暴露（第 4c 项）：层栈、`view` 工具的可见性门控与导航后的界面稳定等待。 */
export { createViewLayer, openViewLayers, refreshViewTools, viewTracker, type GateSpec, type ViewGate } from './tracker'
export { settleView } from './settle'
export { elementVisible } from './visible'
export { checkPageName, inheritedOption, ViewDeclaration, type ScopeChain, type ViewChange, type ViewFields } from './declaration'
